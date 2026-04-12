use crate::cli::start::StartArgs;
use clap::Args;

#[derive(Args, Debug)]
pub struct StartTuiArgs {
    #[command(flatten)]
    pub start: StartArgs,
    #[arg(long, value_name = "LOG_FILE", help = "Additional file to log on")]
    pub log_file: Option<String>,
}
