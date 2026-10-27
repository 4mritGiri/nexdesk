use std::path::PathBuf;

const USAGE: &str = "\
Usage: nexdesk [--connect <NAME>]

  -c, --connect <NAME>  open the saved connection NAME and ask for its password
  -h, --help            show this help
";

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Run { connect: Option<String> },
    Help,
}

fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, String> {
    let mut connect = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "-c" | "--connect" => args
                .next()
                .ok_or_else(|| format!("{arg} needs a connection name"))?,
            other => match other.strip_prefix("--connect=") {
                Some(name) => name.to_string(),
                None => return Err(format!("unknown argument `{other}`")),
            },
        };

        if value.trim().is_empty() {
            return Err("the connection name is empty".into());
        }
        if connect.replace(value).is_some() {
            return Err("--connect can be given only once".into());
        }
    }

    Ok(Parsed::Run { connect })
}

fn engine_path() -> PathBuf {
    let name = if cfg!(windows) {
        "nexdesk-rdp.exe"
    } else {
        "nexdesk-rdp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn main() {
    match parse_args(std::env::args().skip(1)) {
        Ok(Parsed::Help) => print!("{USAGE}"),
        Ok(Parsed::Run { connect }) => nexdesk_ui::app::run(engine_path(), connect),
        Err(message) => {
            eprintln!("nexdesk: {message}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Parsed, String> {
        parse_args(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn no_arguments_just_opens_the_manager() {
        assert_eq!(parse(&[]), Ok(Parsed::Run { connect: None }));
    }

    #[test]
    fn connect_takes_a_name_in_every_spelling() {
        let wanted = Ok(Parsed::Run {
            connect: Some("Office PC".into()),
        });
        assert_eq!(parse(&["--connect", "Office PC"]), wanted);
        assert_eq!(parse(&["-c", "Office PC"]), wanted);
        assert_eq!(parse(&["--connect=Office PC"]), wanted);
    }

    #[test]
    fn a_name_that_looks_like_an_option_is_still_a_name() {
        assert_eq!(
            parse(&["--connect", "--help"]),
            Ok(Parsed::Run {
                connect: Some("--help".into())
            })
        );
    }

    #[test]
    fn mistakes_are_reported_not_ignored() {
        assert!(parse(&["--connect"]).is_err());
        assert!(parse(&["--connect", "  "]).is_err());
        assert!(parse(&["--connect=a", "-c", "b"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert_eq!(parse(&["-h"]), Ok(Parsed::Help));
    }
}
