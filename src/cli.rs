use argh::FromArgs;
use std::fs;

/// mash: control multiple SSH sessions from a single interactive shell
#[derive(FromArgs)]
pub(crate) struct Args {
    /// read hostnames from given file, one per line
    #[argh(option, long = "hosts-file")]
    pub(crate) hosts_filenames: Vec<String>,

    /// command to execute on the remote shells (non-interactive)
    #[argh(option)]
    pub(crate) command: Option<String>,

    /// ssh command template
    #[argh(option, default = "String::from(\"exec ssh -oLogLevel=Quiet -t %(host)s %(port)s\")")]
    pub(crate) ssh: String,

    /// remote user to log in as
    #[argh(option)]
    pub(crate) user: Option<String>,

    /// disable colored hostnames
    #[argh(switch, long = "no-color")]
    pub(crate) no_color: bool,

    /// read a password from the specified file (use - for tty)
    #[argh(option, long = "password-file")]
    pub(crate) password_file: Option<String>,

    /// file to log each machine conversation
    #[argh(option, long = "log-file")]
    pub(crate) log_file: Option<String>,

    /// abort if some shell fails to initialize
    #[argh(switch, long = "abort-errors")]
    pub(crate) abort_errors: bool,

    /// print debugging information
    #[argh(switch)]
    pub(crate) debug: bool,

    /// hostnames to connect to
    #[argh(positional)]
    pub(crate) host_names: Vec<String>,
}

pub(crate) fn parse_args() -> Args {
    let mut args: Args = argh::from_env();

    // Read hosts from files
    for filename in &args.hosts_filenames {
        match fs::read_to_string(filename) {
            Ok(content) => {
                for line in content.lines() {
                    let line = if let Some(idx) = line.find('#') {
                        &line[..idx]
                    } else {
                        line
                    };
                    let line = line.trim();
                    if !line.is_empty() {
                        args.host_names.push(line.to_string());
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading hosts file {}: {}", filename, e);
                std::process::exit(1);
            }
        }
    }

    if args.host_names.is_empty() {
        eprintln!("No hosts given");
        std::process::exit(1);
    }

    args
}
