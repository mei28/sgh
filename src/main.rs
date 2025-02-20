pub mod searchable;
pub mod ssh;
pub mod ssh_config;
pub mod ui;

use anyhow::Result;
use clap::Parser;
use ui::{App, AppConfig};

#[derive(Parser, Debug)]
#[command(version, about, long_about=None)]
struct Args {
    #[arg(
        short,
        long,
        num_args = 1.., 
        default_values = ["/etc/ssh/ssh_config", "~/.ssh/config"]
    )]
    config: Vec<String>,

    // show the proxy command
    #[arg(long, default_value_t = false)]
    show_proxy_command: bool,

    // host search filter
    #[arg(short, long)]
    search: Option<String>,

    // sort hosts by name
    #[arg(long, default_value_t = false)]
    sort: bool,

    // Handlebars template of the command to excute
    #[arg(short, long, default_value = "ssh \"{{{name}}}\"")]
    template: String,

    // Handlebars template of the command to execute when an SSH session starts
    #[arg(long, value_name = "TEMPLATE")]
    on_session_start_template: Option<String>,

    // Handlebars template of the command to execute when an SSH session ends
    #[arg(long, value_name = "TEMPLATE")]
    on_session_end_template: Option<String>,

    // Exit after ending the SSH session
    #[arg(short, long, default_value_t = false)]
    exit: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let mut app = App::new(&AppConfig {
        config_paths: args.config,
        search_filter: args.search,
        sort_by_name: args.sort,
        show_proxy_command: args.show_proxy_command,
        command_template: args.template,
        command_template_on_session_start: args.on_session_start_template,
        command_template_on_session_end: args.on_session_end_template,
        exit_after_ssh_session_ends: args.exit,
    })?;
    app.start()?;

    Ok(())
}
