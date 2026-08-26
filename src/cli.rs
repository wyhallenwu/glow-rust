use std::{
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
};

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

    /// Listen on all IPv4 interfaces so other devices can connect.
    #[arg(long, conflicts_with = "host")]
    pub lan: bool,

    /// Address to bind. Defaults to 127.0.0.1 for local-only access.
    #[arg(long, value_name = "IP")]
    pub host: Option<IpAddr>,

    /// Port to bind; zero selects a free port.
    #[arg(short = 'P', long, default_value_t = 0)]
    pub port: u16,

    /// Open the site in the default browser.
    #[arg(long)]
    pub open: bool,
}

impl ServeArgs {
    /// Resolve the requested listener address while keeping network access
    /// opt-in.
    #[must_use]
    pub fn bind_ip(&self) -> IpAddr {
        if self.lan {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            self.host.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn serve_args(arguments: &[&str]) -> ServeArgs {
        let cli = Cli::try_parse_from(arguments).expect("parse CLI");
        match cli.command {
            Some(Command::Serve(arguments)) => arguments,
            _ => panic!("expected serve command"),
        }
    }

    #[test]
    fn serve_defaults_to_loopback() {
        let arguments = serve_args(&["glow", "serve"]);
        assert_eq!(arguments.bind_ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn lan_mode_binds_all_ipv4_interfaces() {
        let arguments = serve_args(&["glow", "serve", "--lan"]);
        assert_eq!(arguments.bind_ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn serve_accepts_a_specific_interface() {
        let arguments = serve_args(&["glow", "serve", "--host", "192.0.2.8"]);
        assert_eq!(
            arguments.bind_ip(),
            "192.0.2.8".parse::<IpAddr>().expect("test IP")
        );
    }
}
