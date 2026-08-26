use std::{net::IpAddr, path::PathBuf};

use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

use crate::render::Theme;

#[derive(Debug, Parser)]
#[command(
    name = "glow",
    version,
    about = "Render and browse Markdown beautifully",
    long_about = "Render Markdown in the terminal, browse a folder recursively, or publish a read-only documentation site."
)]
pub struct Cli {
    /// Markdown file, directory, URL, repository shortcut, or '-' for stdin.
    #[arg(value_name = "SOURCE|DIR")]
    pub source: Option<String>,

    /// Display rendered output with $PAGER (or less -R).
    #[arg(short, long, global = true)]
    pub pager: bool,

    /// Force the interactive terminal browser.
    #[arg(short, long, global = true)]
    pub tui: bool,

    /// Color theme.
    #[arg(short, long, value_name = "auto|dark|light", global = true)]
    pub style: Option<Theme>,

    /// Maximum render width; zero uses the terminal width.
    #[arg(short, long, global = true)]
    pub width: Option<usize>,

    /// Include hidden and ignored Markdown files.
    #[arg(short, long, global = true)]
    pub all: bool,

    /// Show line numbers in fenced code blocks.
    #[arg(short = 'l', long, global = true)]
    pub line_numbers: bool,

    /// Preserve soft line breaks from the Markdown source.
    #[arg(short = 'n', long, global = true)]
    pub preserve_new_lines: bool,

    /// Enable mouse interactions in the TUI.
    #[arg(short, long, global = true, hide = true)]
    pub mouse: bool,

    /// Read configuration from this YAML file.
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start a responsive local documentation website.
    Serve(ServeArgs),
    /// Publish a folder with a temporary Cloudflare Quick Tunnel.
    Share(ShareArgs),
    /// Print every discovered Markdown document.
    List(ListArgs),
    /// Create, inspect, or edit the configuration file.
    Config(ConfigArgs),
    /// Generate shell completion code.
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Clone, Debug, Args)]
pub struct ServeArgs {
    /// Documentation root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Address to bind. Keep the default for local-only access.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: IpAddr,

    /// Port to bind; zero selects a free port.
    #[arg(short = 'P', long, default_value_t = 0)]
    pub port: u16,

    /// Open the site in the default browser.
    #[arg(long)]
    pub open: bool,
}

#[derive(Clone, Debug, Args)]
pub struct ShareArgs {
    /// Documentation root.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Explicit cloudflared executable path.
    #[arg(long, env = "CLOUDFLARED_BIN")]
    pub cloudflared: Option<PathBuf>,

    /// Seconds to wait for Cloudflare to allocate the public URL.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,

    /// Open the public URL in the default browser.
    #[arg(long)]
    pub open: bool,
}

#[derive(Clone, Debug, Args)]
pub struct ListArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Emit machine-readable JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Clone, Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the active configuration path.
    Path,
    /// Create a documented default configuration file.
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Open the configuration in $EDITOR.
    Edit,
    /// Print the resolved configuration.
    Show,
}
