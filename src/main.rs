use std::path::{Path, PathBuf};
use std::env;
use std::collections::HashMap;
use std::process::{exit, Command, Stdio};
use std::iter;
use std::io::{self, Write};

use freedesktop_entry_parser::parse_entry;
use clap;
use itertools::Itertools;

#[derive(Debug)]
struct ApplicationBody {
    path: PathBuf,
    exec: Vec<String>,
    term: bool,
}

impl ApplicationBody {
    fn new<P: Into<PathBuf>,E: IntoIterator<Item = String>>(path: P, exec: E, term: bool) -> ApplicationBody {
        ApplicationBody {
            path: path.into(),
            exec: exec.into_iter().collect(),
            term,
        }
    }
}

#[derive(Debug)]
struct Application {
    name: String,
    body: ApplicationBody,
}

impl Application {
    fn new<P: Into<PathBuf>, N: Into<String>, E: IntoIterator<Item = String>>(name: N, path: P, exec: E, term: bool) -> Application {
        Application {
            name: name.into(),
            body: ApplicationBody::new(path, exec, term),
        }
    }

    fn exec_from_str<S: AsRef<str>>(execstr: S) -> Vec<String> {
        execstr.as_ref()
            .trim()
            .split_whitespace()
            .filter(|&s| !s.starts_with("%"))
            .map(String::from)
            .collect()
    }

    fn from_file<P: AsRef<Path>>(path: P) -> Option<Application> {
        let desktop_file = parse_entry(path.as_ref()).ok()?;

        if desktop_file.section("Desktop Entry").attr("NoDisplay") != Some("true") { // check if visible
            if desktop_file.section("Desktop Entry").attr("Type") == Some("Application") { // check if app
                let name_o = desktop_file.section("Desktop Entry").attr("Name");
                let execstr_o = desktop_file.section("Desktop Entry").attr("Exec");
                let term: bool = desktop_file.section("Desktop Entry")
                    .attr("Terminal")
                    .unwrap_or("false")
                    .to_lowercase()
                    .parse()
                    .ok()?;

                match (name_o, execstr_o) {
                    (Some(name), Some(execstr)) => {
                        return Some(Application::new(
                                name,
                                path.as_ref(),
                                Application::exec_from_str(execstr),
                                term
                                ));
                    },
                    _ => ()
                }
            } 
        }
        None
    }
}

fn is_desktop_file<P: AsRef<Path>>(path: P) -> Option<PathBuf> {
    if path.as_ref()
        .to_str()
        .unwrap()
        .ends_with("desktop") { 
            if let Ok(location) = path.as_ref().canonicalize() {
                if location.is_file() {
                    return Some(location)
                }
            }
    }
    None
}

fn get_desktop_apps<T: AsRef<Path>>(path: T) -> io::Result<Vec<Application>> {
    Ok(std::fs::read_dir(path.as_ref())?
        .filter_map(|i| if let Ok(s) = i {Some(s.path())} else {None}) // Result<DirEntry> -> PathBuf
        .filter_map(is_desktop_file)
        .filter_map(Application::from_file)
        .collect())
}


fn main() {
    let matches = clap::App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author(env!("CARGO_PKG_AUTHORS").replace(":", ", ").as_str())
        .about(
"searches desktop files and lets the user launch an
application using a menu of their choice, such as dmenu")
        .arg(clap::Arg::with_name("terminal emulator")
            .short("t")
            .long("term")
            .value_name("TERMINAL EMULATOR")
            .help("sets the terminal emulator, defaults to $TERM")
            .takes_value(true))
        .arg(clap::Arg::with_name("MENU PROGRAM")
            .help("the menu program to be used, such as dmenu")
            .required(true)
            .index(1)
            )
        .arg(clap::Arg::with_name("searchdirs")
            .help(
"the directories to be searched, defaults to
/usr/share/applications and ~/.local/share/applications")
            .index(2)
            .multiple(true)
            .required(false)
            )
        .get_matches();

    let menu_program: Vec<_> = matches.value_of("MENU PROGRAM").unwrap()
        .split_whitespace()
        .collect();

    let apps_repos: Vec<io::Result<Vec<Application>>> = match matches.values_of("searchdirs") {
        Some(searchdirs) => {
            searchdirs.map(get_desktop_apps)
                .collect()
        },
        None => {
            let mut h = PathBuf::new(); // doesn't like ~/.local/share/applications for some reason
            h.push(env::var("HOME").unwrap());
            h.push(".local");
            h.push("share");
            h.push("applications");

            vec![
                get_desktop_apps("/usr/share/applications"),
                get_desktop_apps(h),
            ]
        }
    };

    let apps_map: HashMap<_, _> = apps_repos.into_iter()
        .filter_map(Result::ok)
        .flatten()
        .map(|i| (i.name, i.body))
        .collect();

    let mut app_names: Vec<&str> = apps_map.iter()
        .map(|i| &**i.0)
        .collect();

    app_names.sort();

    let newlines = iter::repeat("\n").take(app_names.len());

    let menu_process_stdin = app_names.into_iter().interleave(newlines).collect::<Vec<&str>>().concat();


    let mut menu_process = Command::new(&menu_program[0]);
    for i in 1..menu_program.len() {
        menu_process.arg(menu_program[i]);
    }

    let mut menu_process = match menu_process.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn() {
            Ok(child) => child,
            Err(e) => {
                eprintln!("failed to spawn menu '{}': {}", menu_program.join(" "), e);
                exit(1);
            }
    };

    {
        menu_process.stdin
            .take()
            .unwrap()
            .write(
                menu_process_stdin.as_bytes())
            .unwrap();
    }

    let selected_program = match menu_process.wait_with_output() {
        Ok(o) => {
            if !o.status.success() {
                exit(0);
            }
            let select = std::str::from_utf8(&o.stdout).unwrap().trim().to_string();
            match select.as_ref() {
                "" => exit(0),
                _ => select
            }
        },
        Err(e) => {
            eprintln!("error: {}", e);
            exit(1);
        }
    };

    println!("chosen program: {}", selected_program);

    let program = &apps_map[&selected_program];

    if program.term {
        let terminal_emulator = {
            match matches.value_of("terminal emulator") {
                Some(t) => t.to_string(),
                None => match env::var("TERM") {
                    Ok(t) => t,
                    Err(_) => {
                        eprintln!("could not infer terminal emulator, assuming xterm");
                        "xterm".to_string()
                    }
                }
            }
        };

        let mut process = Command::new(&terminal_emulator);
        process.arg("-e");
        for i in program.exec.iter() {
            process.arg(i);
        }

        match process.output() {
            Err(e) => {
                eprintln!("error when executing '{} -e {}': {}", &terminal_emulator, program.exec.join(" "), e);
                exit(1);
            },
            _ => ()
        }
        
        
    }
    else {
        let mut process = Command::new(&program.exec[0]);
        for i in 1..program.exec.len() {
            process.arg(&program.exec[i]);
        }

        match process.output() {
            Err(e) => {
                eprintln!("error when executing '{}': {}", program.exec.join(" "), e);
                exit(1);
            },
            _ => ()
        }
    }
}
