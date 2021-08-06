// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2019-2020 Normation SAS

use crate::logs::LogLevel;
use serde::Deserialize;
use std::{cmp::PartialEq, path::PathBuf};
use structopt::StructOpt;

#[derive(Clone, Debug, StructOpt, Deserialize, PartialEq)]
#[structopt(rename_all = "kebab-case")]
pub struct Options {
    /// Path of the configuration file to use.
    /// A configuration file is required (containing at least stdlib and generic_methods paths)
    #[structopt(long, short, default_value = "/opt/rudder/etc/rudderc.conf")]
    pub config_file: PathBuf,

    /// Input file path.
    ///
    /// If option path does not exist, concat config input with option.
    #[structopt(long, short)]
    pub input: Option<PathBuf>,

    /// Output file path.
    ///
    /// If option path does not exist, concat config output with option.
    ///
    ///Else base output on input.
    #[structopt(long, short)]
    pub output: Option<PathBuf>,

    /// rudderc output logs verbosity.
    #[structopt(
        long,
        short,
        possible_values = &["off", "trace", "debug", "info", "warn", "error"],
        default_value = "warn"
    )]
    pub log_level: LogLevel,

    /// Takes stdin as an input rather than using a file. Overwrites input file option
    #[structopt(long)]
    pub stdin: bool,

    /// Takes stdout as an output rather than using a file. Overwrites output file option. Dismiss logs directed to stdout.
    /// Errors are kept since they are printed to stderr
    #[structopt(long)]
    pub stdout: bool,

    /// Use json logs instead of human readable output
    ///
    /// This option will print a single JSON object that will contain logs, errors and generated data (or the file where it has been generated)
    ///
    /// JSON output format is always the same, whichever command is chosen.
    /// However, some fields (data and destination file) could be set to `null`, make sure to handle `null`s properly.
    ///
    /// Note that NO_COLOR specs apply by default for json output.
    ///
    /// Also note that setting NO_COLOR manually in your env will also work
    #[structopt(long, short)]
    pub json_logs: bool,

    /// Generates a backtrace in case an error occurs
    #[structopt(long, short)]
    pub backtrace: bool,
}
