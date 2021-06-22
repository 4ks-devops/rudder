// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2019-2020 Normation SAS

#[macro_use]
pub mod logs;
pub mod cli_parser;
pub mod output;

use self::cli_parser::Options;
use crate::{command::Command, error::*, generator::Format, parser::Token};
use colored::Colorize;
use serde::Deserialize;
use std::{fmt, fs, io::Read, path::PathBuf, str::FromStr};

#[derive(Clone, Debug, Deserialize)]
struct LibsConfig {
    stdlib: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
struct IOPaths {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
}

// deserialized config file main struct
#[derive(Clone, Debug, Deserialize)]
struct Config {
    #[serde(rename = "shared")]
    libs: LibsConfig,
    compile: IOPaths,
    lint: IOPaths,
    save: IOPaths,
    technique_generate: IOPaths,
    technique_read: IOPaths,
}

/// IO context deduced from arguments (structopt) and config file (Config)
// must always reflect Options + add unique fields
#[derive(Clone, Debug, PartialEq)]
pub struct IOContext {
    // GenericOption reflection
    pub stdlib: PathBuf,
    pub input: String,
    pub input_content: String,
    pub output: Option<PathBuf>,
    // Unique fields
    pub format: Format,
    pub command: Command,
}
// TODO might try to merge io.rs in here
impl IOContext {
    pub fn new(command: Command, opt: &Options, format: Option<Format>) -> Result<Self> {
        let config: Config = match std::fs::read_to_string(&opt.config_file) {
            Err(e) => {
                return Err(Error::new(format!(
                    "Could not read toml config file: {}",
                    e
                )))
            }
            Ok(config_data) => match toml::from_str(&config_data) {
                Ok(config) => config,
                Err(e) => {
                    return Err(Error::new(format!(
                        "Could not parse (probably faulty) toml config file: {}",
                        e
                    )))
                }
            },
        };

        let command_config = match command {
            Command::Compile => config.compile,
            Command::GenerateTechnique => config.technique_generate,
            Command::Save => config.save,
            Command::ReadTechnique => config.technique_read,
            Command::Lint => config.lint,
        };
        let (input, input_str, input_content) =
            get_input(&command_config.input, &opt.input, opt.stdin)?;
        let (output, format) = get_output(
            &command_config.output,
            command,
            &input,
            &opt.output,
            opt.stdout,
            format,
        )?;

        let ctx = Self {
            stdlib: config.libs.stdlib.clone(),
            input: input_str,
            input_content,
            output,
            command,
            format,
        };
        info!("I/O context: {}", ctx);

        Ok(ctx)
    }

    pub fn with_content(&self, input_content: String) -> Self {
        Self {
            input_content,
            ..self.clone()
        }
    }

    pub fn with_format(&self, format: Format) -> Self {
        Self {
            format,
            ..self.clone()
        }
    }

    pub fn with_input(&self, input: &str) -> Self {
        Self {
            input: input.to_owned(),
            ..self.clone()
        }
    }
}
impl fmt::Display for IOContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            &format!(
                "{} of {:?} into {:?}. Output format is {}. Libraries path: {:?}.",
                self.command, self.input, self.output, self.format, self.stdlib,
            )
        )
    }
}

/// If stdin, returns None. Else if input and config lead to existing file, Some(file). Else, error.
fn get_input(
    config_path: &Option<PathBuf>,
    input: &Option<PathBuf>,
    is_stdin: bool,
) -> Result<(Option<PathBuf>, String, String)> {
    if is_stdin {
        if input.is_some() {
            info!("Input file not used because of the --stdin option");
        }
        let (input_str, content) = get_content(&None)?;
        return Ok((None, input_str, content));
    }
    let input = Some(match (input, config_path) {
        (Some(i), _) if i.is_file() => i.to_owned(),
        (Some(i), Some(c)) => {
            let path = c.join(i);
            if path.is_file() {
                path.to_owned()
            } else {
                return Err(Error::new(
                    "Commands: input does not match any existing file".to_owned(),
                ));
            }
        }
        (None, Some(c)) if c.is_file() => c.to_owned(),
        _ => {
            return Err(Error::new(
                "Commands: no input or input does not match any existing file".to_owned(),
            ))
        }
    });
    let (input_str, content) = get_content(&input)?;
    Ok((input, input_str, content))
}

/// get explicit output file
fn get_output(
    config_path: &Option<PathBuf>,
    command: Command,
    input: &Option<PathBuf>,
    argv_output: &Option<PathBuf>,
    is_stdout: bool,
    format: Option<Format>,
) -> Result<(Option<PathBuf>, Format)> {
    if is_stdout && argv_output.is_some() {
        warn!("commands: stdout option conflicts with output option. Priority to the former.");
    }
    // is_stdout OR exception for Generate Technique which is designed to work from stdin: default stdout unless output specified
    if is_stdout || (command == Command::GenerateTechnique && argv_output == &None) {
        return Ok((None, get_output_format(command, format, &None)?.1));
    }

    let technique = Some(match (&argv_output, config_path, input) {
        (Some(o), _, _) if o.parent().filter(|p| p.is_dir()).is_some() => o.to_owned(),
        (Some(o), Some(c), _) => {
            let path = c.join(o);
            if path.parent().filter(|p| p.is_dir()).is_some() {
                path
            } else {
                return Err(Error::new(
                    "Commands: paths do not match any existing directory".to_owned(),
                ));
            }
        }
        (None, Some(c), _) if c.is_file() => c.to_owned(),
        (_, _, Some(i)) => i.to_owned(),
        (_, _, None) => {
            return Err(Error::new(format!(
                "Commands: no parameters or configuration output or input to base output file on"
            )))
        }
    });

    // format is part of output file so it makes sense to return it from this function plus it needs to be defined here to update output if needed
    let (format_as_str, format) = get_output_format(command, format, &technique)?;
    Ok((
        technique.map(|output| output.with_extension(&format_as_str)),
        format,
    ))
}

/// get explicit output. If no explicit output get default path + filename. I none, use input path (and update format). If none worked, error
fn get_output_format(
    command: Command,
    format: Option<Format>,
    output: &Option<PathBuf>,
) -> Result<(String, Format)> {
    if command == Command::Compile && format.is_some() {
        info!("Command line format option used");
    }

    // All formats but Compile are hardcoded in CLI implementation, so this is partly double check
    match (command, format) {
        (Command::Compile, Some(fmt))
            if fmt == Format::CFEngine || fmt == Format::DSC || fmt == Format::Markdown =>
        {
            Ok((format!("{}.{}", "rd", fmt), fmt))
        }
        (Command::Compile, _) => {
            info!("Commands: missing or invalid format, deducing it from output file extension");
            let ext = match output {
                Some(o) => o.extension(),
                None => None,
            };
            return match ext.and_then(|fmt| fmt.to_str()) {
                Some(fmt) => {
                    let fmt = Format::from_str(fmt)?;
                    Ok((format!("{}", fmt), fmt))
                }
                None => Err(Error::new(
                    "Commands: missing or invalid format, plus unrecognized or invalid output file extension".to_owned(),
                ))
            };
        }
        (Command::ReadTechnique, Some(fmt)) => Ok((format!("{}.{}", "rd", fmt), fmt)),
        (_, Some(fmt)) => Ok((format!("{}", fmt), fmt)),
        (_, None) => {
            panic!("Commands: format should have been defined earlier in program execution")
        }
    }
}

/// Add a single file content to the sources and parse it
/// Returns the filename and file content
pub fn get_content<'src>(path: &Option<PathBuf>) -> Result<(String, String)> {
    match path {
        Some(file_path) => {
            // TODO check: is full path required or is filename-only better (cleaner)?
            let filename = match file_path.file_name() {
                Some(file) => {
                    let file = file.to_string_lossy().to_string();
                    info!(
                        "|- {} from {}",
                        "Reading".bright_green(),
                        file.bright_yellow()
                    );
                    file
                }
                None => {
                    return Err(Error::new(format!(
                        "{:?} file does not exist or is invalid",
                        path
                    )))
                }
            };
            match fs::read_to_string(file_path) {
                Ok(content) => Ok((filename, content)),
                Err(e) => Err(err!(Token::new(&filename, ""), "{}", e)),
            }
        }
        // None means expect input from STDIN (see CLI methods)
        None => {
            let mut buffer = String::new();
            match std::io::stdin().read_to_string(&mut buffer) {
                Ok(_) => {
                    info!(
                        "|- {} from {}",
                        "Reading".bright_green(),
                        "STDIN".bright_yellow()
                    );
                    Ok(("STDIN".to_owned(), buffer))
                }
                Err(e) => Err(err!(Token::new("STDIN", ""), "{}", e)),
            }
        }
    }
}
