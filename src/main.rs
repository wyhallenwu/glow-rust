use std::{
    env,
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{CommandFactory, Parser};
use glow::{
    cli::{Cli, Command, ConfigAction},
    config::{Config, default_config_path},
    discover::{DocumentIndex, ScanOptions},
    document::is_markdown,
    render::terminal::{TerminalRenderOptions, render_ansi},
    source,
    tui::{self, TuiOptions},
    tunnel::TunnelOptions,
    web::{self, ServeOptions, ShareOptions},
};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let config_command = matches!(&cli.command, Some(Command::Config(_)));
    let explicit_missing = cli.config.as_ref().is_some_and(|path| !path.exists());
    let mut config = if config_command && explicit_missing {
        Config::default()
    } else {
        Config::load(cli.config.as_deref())?
    };
    apply_cli_overrides(&cli, &mut config)?;
    let explicit_config = cli.config.clone();

    if let Some(command) = cli.command {
        return run_command(command, &config, explicit_config.as_deref()).await;
    }

    let piped_stdin = !io::stdin().is_terminal();
    let source_arg = if piped_stdin {
        Some("-".to_owned())
    } else {
        cli.source
    };

    match source_arg {
        None => run_tui(Path::new("."), &config),
        Some(argument) => {
            let path = Path::new(&argument);
            if path.is_dir() {
                return run_tui(path, &config);
            }
            if config.tui && path.is_file() {
                return run_tui(path.parent().unwrap_or_else(|| Path::new(".")), &config);
            }
            render_source(&argument, &config).await
        }
    }
}

fn apply_cli_overrides(cli: &Cli, config: &mut Config) -> Result<()> {
    config.pager |= cli.pager;
    config.tui |= cli.tui;
    config.include_hidden |= cli.all;
    config.line_numbers |= cli.line_numbers;
    config.preserve_newlines |= cli.preserve_new_lines;
    config.mouse |= cli.mouse;
    if let Some(theme) = cli.style {
        config.theme = theme;
    }
    if let Some(width) = cli.width {
        config.width = width;
    }
    if config.pager && config.tui {
        bail!("cannot use both --pager and --tui");
    }
    Ok(())
}

async fn run_command(
    command: Command,
    config: &Config,
    explicit_config: Option<&Path>,
) -> Result<()> {
    match command {
        Command::Serve(arguments) => {
            let bind_ip = arguments.bind_ip();
            let mut options = ServeOptions::new(arguments.path)
                .with_bind_ip(bind_ip)
                .with_port(arguments.port)
                .with_hidden(config.include_hidden);
            options.respect_gitignore = !config.include_hidden;
            if arguments.open {
                serve_and_open(options).await
            } else {
                web::serve(options).await
            }
        }
        Command::Share(arguments) => {
            let mut options = ShareOptions::new(arguments.path);
            options.server.include_hidden = config.include_hidden;
            options.server.respect_gitignore = !config.include_hidden;
            options.tunnel = TunnelOptions {
                executable: arguments
                    .cloudflared
                    .map_or_else(|| OsString::from("cloudflared"), PathBuf::into_os_string),
                startup_timeout: Duration::from_secs(arguments.timeout),
            };
            options.open_browser = arguments.open;
            web::share(options).await
        }
        Command::List(arguments) => list_documents(&arguments.path, arguments.json, config),
        Command::Config(arguments) => run_config_action(arguments.action, config, explicit_config),
        Command::Completion { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "glow", &mut io::stdout());
            Ok(())
        }
    }
}

async fn serve_and_open(options: ServeOptions) -> Result<()> {
    let mut server = web::spawn(options).await?;
    web::announce(&server);
    let url = server.browser_url();
    open_browser(&url)?;
    println!("Press Ctrl-C to stop.");
    let reason = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.context("cannot listen for Ctrl-C"),
        stopped = server.stopped() => stopped.and_then(|()| Err(anyhow!("documentation server stopped unexpectedly"))),
    };
    let shutdown = server.shutdown().await;
    reason.and(shutdown)
}

fn run_tui(root: &Path, config: &Config) -> Result<()> {
    if !io::stdout().is_terminal() {
        bail!(
            "the folder browser needs an interactive terminal; use `glow list {}` instead",
            root.display()
        );
    }
    tui::run(
        root,
        TuiOptions {
            include_hidden: config.include_hidden,
            respect_gitignore: !config.include_hidden,
            theme: config.theme,
            line_numbers: config.line_numbers,
            mouse: config.mouse,
        },
    )
}

async fn render_source(argument: &str, config: &Config) -> Result<()> {
    let loaded = source::load(argument).await?;
    let markdown = if source_name_is_markdown(&loaded.name) {
        loaded.content
    } else {
        let language = Path::new(&loaded.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("text");
        format!("```{language}\n{}\n```", loaded.content)
    };
    let color = io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none();
    let output = render_ansi(
        &markdown,
        TerminalRenderOptions {
            width: render_width(config.width),
            theme: config.theme,
            line_numbers: config.line_numbers,
            preserve_newlines: config.preserve_newlines,
        },
        color,
    );
    if config.pager {
        run_pager(&output)
    } else {
        print!("{output}");
        io::stdout().flush().context("cannot flush rendered output")
    }
}

fn source_name_is_markdown(name: &str) -> bool {
    let path = Path::new(name);
    path.extension().is_none() || is_markdown(path)
}

fn render_width(configured: usize) -> usize {
    if configured > 0 {
        return configured.max(8);
    }
    crossterm::terminal::size()
        .map(|(width, _)| usize::from(width).min(120))
        .unwrap_or(80)
        .max(8)
}

fn run_pager(output: &str) -> Result<()> {
    let pager = env::var("PAGER").unwrap_or_else(|_| "less -R".to_owned());
    let fields = shell_words::split(&pager).context("cannot parse $PAGER")?;
    let Some((program, arguments)) = fields.split_first() else {
        bail!("$PAGER is empty");
    };
    let mut child = ProcessCommand::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot start pager {program}"))?;
    child
        .stdin
        .take()
        .context("pager stdin is unavailable")?
        .write_all(output.as_bytes())
        .context("cannot write to pager")?;
    let status = child.wait().context("cannot wait for pager")?;
    if status.success() {
        Ok(())
    } else {
        bail!("pager exited with {status}")
    }
}

fn list_documents(path: &Path, json: bool, config: &Config) -> Result<()> {
    let index = DocumentIndex::scan(
        path,
        &ScanOptions {
            include_hidden: config.include_hidden,
            respect_gitignore: !config.include_hidden,
        },
    )?;
    if json {
        let values = index
            .documents
            .iter()
            .map(|document| {
                serde_json::json!({
                    "path": document.relative_path.to_string_lossy(),
                    "title": document.title,
                    "size": document.size,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        for document in &index.documents {
            println!("{}\t{}", document.relative_path.display(), document.title);
        }
    }
    Ok(())
}

fn run_config_action(
    action: Option<ConfigAction>,
    config: &Config,
    explicit_config: Option<&Path>,
) -> Result<()> {
    let path = explicit_config.map_or_else(default_config_path, Path::to_path_buf);
    match action.unwrap_or(ConfigAction::Edit) {
        ConfigAction::Path => {
            println!("{}", path.display());
            Ok(())
        }
        ConfigAction::Init { force } => {
            Config::default().save(&path, force)?;
            println!("Created {}", path.display());
            Ok(())
        }
        ConfigAction::Edit => {
            if !path.exists() {
                Config::default().save(&path, false)?;
            }
            open_editor(&path)
        }
        ConfigAction::Show => {
            print!("{}", serde_yaml::to_string(config)?);
            Ok(())
        }
    }
}

fn open_editor(path: &Path) -> Result<()> {
    let editor = env::var("VISUAL")
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_owned()
            } else {
                "vi".to_owned()
            }
        });
    let fields = shell_words::split(&editor).context("cannot parse $EDITOR")?;
    let Some((program, arguments)) = fields.split_first() else {
        bail!("$EDITOR is empty");
    };
    let status = ProcessCommand::new(program)
        .args(arguments)
        .arg(path)
        .status()
        .with_context(|| format!("cannot start editor {program}"))?;
    if status.success() {
        Ok(())
    } else {
        bail!("editor exited with {status}")
    }
}

fn open_browser(url: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = ProcessCommand::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot open the default browser")?;
    Ok(())
}
