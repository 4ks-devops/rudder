// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2019-2020 Normation SAS

use crate::{
    configuration::main::ReportingOutputSelect,
    data::{RunInfo, RunLog},
    error::Error,
    input::{read_compressed_file, signature, watch::*},
    output::{
        database::{insert_runlog, InsertionBehavior},
        upstream::send_report,
    },
    processing::{failure, success, OutputError, ReceivedFile},
    stats::Event,
    JobConfig,
};
use futures::{
    future::{poll_fn, Future},
    lazy,
    sync::mpsc,
    Stream,
};
use md5::{Digest, Md5};
use std::{convert::TryFrom, os::unix::ffi::OsStrExt, sync::Arc};
use tokio::prelude::*;
use tokio_threadpool::blocking;
use tracing::{debug, error, info, span, warn, Level};

static REPORT_EXTENSIONS: &[&str] = &["gz", "zip", "log"];

pub fn start(job_config: &Arc<JobConfig>, stats: &mpsc::Sender<Event>) {
    let span = span!(Level::TRACE, "reporting");
    let _enter = span.enter();

    let incoming_path = job_config
        .cfg
        .processing
        .reporting
        .directory
        .join("incoming");
    let failed_path = job_config.cfg.processing.reporting.directory.join("failed");

    let (sender, receiver) = mpsc::channel(1_024);
    tokio::spawn(cleanup(
        incoming_path.clone(),
        job_config.cfg.processing.reporting.cleanup,
    ));
    tokio::spawn(cleanup(
        failed_path.clone(),
        job_config.cfg.processing.reporting.cleanup,
    ));
    tokio::spawn(serve(job_config.clone(), receiver, stats.clone()));
    watch(
        &incoming_path,
        job_config.cfg.processing.reporting.catchup,
        &sender,
    );
}

fn serve(
    job_config: Arc<JobConfig>,
    rx: mpsc::Receiver<ReceivedFile>,
    stats: mpsc::Sender<Event>,
) -> impl Future<Item = (), Error = ()> {
    rx.for_each(move |file| {
        // allows skipping temporary .dav files
        if !file
            .extension()
            .map(|f| REPORT_EXTENSIONS.contains(&f.to_string_lossy().as_ref()))
            .unwrap_or(false)
        {
            debug!(
                "skipping {:#?} as it does not have a known report extension",
                file
            );
            return Ok(());
        }

        let queue_id = format!(
            "{:X}",
            Md5::digest(
                file.file_name()
                    .unwrap_or_else(|| file.as_os_str())
                    .as_bytes()
            )
        );
        let span = span!(
            Level::INFO,
            "report",
            queue_id = %queue_id,
        );
        let _enter = span.enter();

        let stat_event = stats
            .clone()
            .send(Event::ReportReceived)
            .map_err(|e| error!("receive error: {}", e))
            .map(|_| ());
        // FIXME: no need for a spawn
        tokio::spawn(lazy(|| stat_event));

        // Check run info
        let info = RunInfo::try_from(file.as_ref()).map_err(|e| warn!("received: {}", e))?;

        let node_span = span!(
            Level::INFO,
            "node",
            node_id = %info.node_id,
        );
        let _node_enter = node_span.enter();

        if !job_config
            .nodes
            .read()
            .expect("Cannot read nodes list")
            .is_subnode(&info.node_id)
        {
            let fail = failure(
                file,
                job_config.cfg.processing.reporting.directory.clone(),
                Event::ReportRefused,
                stats.clone(),
            );

            // FIXME: no need for a spawn
            tokio::spawn(lazy(|| fail));
            error!("refused: report from {:?}, unknown id", &info.node_id);
            // this is actually expected behavior
            return Ok(());
        }

        debug!("received: {:?}", file);

        let treat_file: Box<dyn Future<Item = (), Error = ()> + Send> =
            match job_config.cfg.processing.reporting.output {
                ReportingOutputSelect::Database => {
                    output_report_database(file, info, job_config.clone(), stats.clone())
                }
                ReportingOutputSelect::Upstream => {
                    output_report_upstream(file, job_config.clone(), stats.clone())
                }
                // The job should not be started in this case
                ReportingOutputSelect::Disabled => unreachable!("Report server should be disabled"),
            };

        tokio::spawn(lazy(|| treat_file));
        Ok(())
    })
}

fn output_report_database(
    path: ReceivedFile,
    run_info: RunInfo,
    job_config: Arc<JobConfig>,
    stats: mpsc::Sender<Event>,
) -> Box<dyn Future<Item = (), Error = ()> + Send> {
    // Everything here is blocking: reading on disk or inserting into database
    // We could use tokio::fs but it works the same and only makes things
    // more complicated.
    // We can switch to it once we also have stream (i.e. not on disk) input.
    let job_config_clone = job_config.clone();
    let path_clone = path.clone();
    let path_clone2 = path.clone();
    let stats_clone = stats.clone();
    Box::new(
        poll_fn(move || {
            blocking(|| {
                output_report_database_inner(&path_clone.clone(), &run_info, &job_config).map_err(
                    |e| {
                        error!("output error: {}", e);
                        OutputError::from(e)
                    },
                )
            })
            .map_err(|_| {
                panic!("the thread pool shut down");
                // Temp hack to fix typing
                #[allow(unreachable_code)]
                OutputError::Transient
            })
        })
        .flatten()
        .or_else(move |e| match e {
            OutputError::Permanent => failure(
                path_clone2.clone(),
                job_config_clone
                    .clone()
                    .cfg
                    .processing
                    .reporting
                    .directory
                    .clone(),
                Event::ReportRefused,
                stats,
            ),
            OutputError::Transient => {
                info!("transient error, skipping");
                Box::new(futures::future::err::<(), ()>(()))
            }
        })
        .and_then(move |_| success(path.clone(), Event::ReportInserted, stats_clone)),
    )
}

fn output_report_upstream(
    path: ReceivedFile,
    job_config: Arc<JobConfig>,
    stats: mpsc::Sender<Event>,
) -> Box<dyn Future<Item = (), Error = ()> + Send> {
    let job_config_clone = job_config.clone();
    let path_clone2 = path.clone();
    let stats_clone = stats.clone();
    Box::new(
        send_report(job_config, path.clone())
            .map_err(|e| {
                error!("output error: {}", e);
                OutputError::from(e)
            })
            .or_else(move |e| match e {
                OutputError::Permanent => failure(
                    path_clone2.clone(),
                    job_config_clone
                        .clone()
                        .cfg
                        .processing
                        .reporting
                        .directory
                        .clone(),
                    Event::ReportRefused,
                    stats,
                ),
                OutputError::Transient => {
                    info!("transient error, skipping");
                    Box::new(futures::future::err::<(), ()>(()))
                }
            })
            .and_then(move |_| success(path.clone(), Event::ReportSent, stats_clone)),
    )
}

fn output_report_database_inner(
    path: &ReceivedFile,
    run_info: &RunInfo,
    job_config: &Arc<JobConfig>,
) -> Result<(), Error> {
    debug!("Starting insertion of {:#?}", path);

    let signed_runlog = signature(
        &read_compressed_file(&path)?,
        job_config
            .clone()
            .nodes
            .read()
            .expect("read nodes")
            .certs(&run_info.node_id)
            .ok_or_else(|| Error::MissingCertificateForNode(run_info.node_id.clone()))?,
    )?;

    let parsed_runlog = RunLog::try_from((run_info.clone(), signed_runlog.as_ref()))?;

    let filtered_runlog = if !job_config
        .cfg
        .processing
        .reporting
        .skip_event_types
        .is_empty()
    {
        parsed_runlog.without_types(&job_config.cfg.processing.reporting.skip_event_types)
    } else {
        parsed_runlog
    };

    let _inserted = insert_runlog(
        &job_config
            .pool
            .clone()
            .expect("output uses database but no config provided"),
        &filtered_runlog,
        InsertionBehavior::SkipDuplicate,
    )?;
    Ok(())
}
