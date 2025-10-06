use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use comically::{Comic, ComicConfig, OutputFormat};

use crate::tui::progress::{ComicStage, ComicStatus, ProgressEvent};
use crate::Event;

pub fn process_files(
    files: Vec<PathBuf>,
    config: ComicConfig,
    output_dir: PathBuf,
    event_tx: mpsc::Sender<Event>,
) {
    log::info!("processing with config: {:?}", config);
    log::info!("processing {} files", files.len());

    let (kindlegen_tx, kindlegen_rx) =
        mpsc::channel::<(usize, PathBuf, PathBuf, mpsc::Sender<Event>)>();

    if config.output_format == OutputFormat::Mobi {
        let event_tx = event_tx.clone();
        thread::spawn(move || {
            poll_kindlegen(kindlegen_rx);
            // after all the comics have finished conversion to mobi, send the complete event
            send_processing_complete(&event_tx);
        });
    }

    let comics: Vec<_> = files
        .into_iter()
        .enumerate()
        .map(|(id, file)| {
            let comic = Comic::new(file);
            send_register_comic(&event_tx, id, comic.title.clone());
            (id, comic)
        })
        .collect();

    // Reusable buffer for building archives - avoids repeated allocations
    let mut build_buffer = Vec::new();

    for (id, comic) in comics {
        // Process images
        let start = Instant::now();
        let archive_iter = match comically::archive::unarchive_comic_iter(&comic.input) {
            Ok(iter) => iter,
            Err(e) => {
                log::error!("Error in comic: {} {e}", comic.title);
                error(&event_tx, id, e);
                continue;
            }
        };

        let num_images = archive_iter.num_images();
        send_comic_update(
            &event_tx,
            id,
            ComicStatus::ImageProcessingStart {
                total_images: num_images,
                start,
            },
        );

        // Collect archive files
        let files: Vec<_> = archive_iter
            .filter_map(|result| {
                result
                    .map_err(|e| log::warn!("Failed to load archive file: {}", e))
                    .ok()
            })
            .collect();

        let on_processed = || {
            send_comic_update(&event_tx, id, ComicStatus::ImageProcessed);
        };

        let images =
            match comically::image::process_batch_with_progress(files, &config, on_processed) {
                Ok(imgs) => imgs,
                Err(e) => {
                    log::error!("Error processing images for {}: {e}", comic.title);
                    error(&event_tx, id, e);
                    continue;
                }
            };

        send_comic_update(
            &event_tx,
            id,
            ComicStatus::ImageProcessingComplete {
                duration: start.elapsed(),
            },
        );

        log::info!("Processed {} images for {}", images.len(), comic.title);

        // Build output format
        let build_start = Instant::now();
        send_comic_update(
            &event_tx,
            id,
            ComicStatus::Progress {
                stage: ComicStage::Package,
                progress: 75.0,
                start: build_start,
            },
        );

        let build_result = match config.output_format {
            OutputFormat::Cbz => {
                comically::cbz::build_into(&comic, &images, &mut build_buffer);

                let output_path = output_dir.join(comic.output_filename(config.output_format));
                std::fs::write(&output_path, &build_buffer)
                    .inspect(|_| log::info!("Created CBZ: {:?}", output_path))
                    .map_err(|e| anyhow::anyhow!("Failed to write CBZ: {}", e))
            }
            OutputFormat::Epub => {
                comically::epub::build_into(&comic, &config, &images, &mut build_buffer);
                let output_path = output_dir.join(comic.output_filename(config.output_format));
                std::fs::write(&output_path, &build_buffer)
                    .inspect(|_| log::info!("Created EPUB: {:?}", output_path))
                    .map_err(|e| anyhow::anyhow!("Failed to write EPUB: {}", e))
            }
            OutputFormat::Mobi => {
                comically::epub::build_into(&comic, &config, &images, &mut build_buffer);

                let epub_path = output_dir.join(comic.output_filename(OutputFormat::Epub));
                std::fs::write(&epub_path, &build_buffer)
                    .inspect(|_| {
                        log::info!("Created EPUB for MOBI: {:?}", epub_path);
                        let output_mobi =
                            output_dir.join(comic.output_filename(OutputFormat::Mobi));
                        kindlegen_tx
                            .send((id, epub_path, output_mobi, event_tx.clone()))
                            .ok();
                    })
                    .map_err(|e| anyhow::anyhow!("Failed to write EPUB: {}", e))
            }
        };

        match build_result {
            Ok(_) => {
                // For MOBI, we continue to kindlegen processing
                if config.output_format != OutputFormat::Mobi {
                    send_comic_update(
                        &event_tx,
                        id,
                        ComicStatus::StageCompleted {
                            stage: ComicStage::Package,
                            duration: build_start.elapsed(),
                        },
                    );
                    send_comic_update(&event_tx, id, ComicStatus::Success);
                }
            }
            Err(e) => {
                log::error!("Error building output for {}: {e}", comic.title);
                error(&event_tx, id, e);
            }
        }
    }

    match config.output_format {
        OutputFormat::Epub | OutputFormat::Cbz => {
            send_processing_complete(&event_tx);
        }
        _ => {}
    }
}

pub fn poll_kindlegen(tx: mpsc::Receiver<(usize, PathBuf, PathBuf, mpsc::Sender<Event>)>) {
    struct KindleGenStatus {
        id: usize,
        spawned: comically::mobi::SpawnedKindleGen,
        start: Instant,
        event_tx: mpsc::Sender<Event>,
    }

    let mut pending = Vec::<Option<KindleGenStatus>>::new();

    'outer: loop {
        loop {
            let result = tx.try_recv();

            match result {
                Ok((id, epub_path, output_mobi, event_tx)) => {
                    let start = Instant::now();
                    send_comic_update(
                        &event_tx,
                        id,
                        ComicStatus::Progress {
                            stage: ComicStage::Convert,
                            progress: 75.0,
                            start,
                        },
                    );

                    match comically::mobi::create(epub_path, output_mobi) {
                        Ok(spawned) => {
                            pending.push(Some(KindleGenStatus {
                                id,
                                spawned,
                                start,
                                event_tx,
                            }));
                        }
                        Err(e) => {
                            log::error!("Error creating MOBI: {e}");
                            send_comic_update(&event_tx, id, ComicStatus::Failed { error: e });
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if pending.is_empty() {
                        break 'outer;
                    } else {
                        break;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    break;
                }
            }
        }

        for s in pending.iter_mut() {
            let is_done = match s {
                Some(status) => match status.spawned.try_wait() {
                    Ok(Some(_)) => true,
                    Ok(None) => false,
                    Err(e) => {
                        log::error!("error waiting for kindlegen: {}", e);
                        true
                    }
                },
                _ => false,
            };

            if is_done {
                if let Some(status) = s.take() {
                    log::debug!("KindleGen process completed");
                    match status.spawned.wait() {
                        Ok(_) => {
                            send_comic_update(
                                &status.event_tx,
                                status.id,
                                ComicStatus::StageCompleted {
                                    stage: ComicStage::Convert,
                                    duration: status.start.elapsed(),
                                },
                            );
                            send_comic_update(&status.event_tx, status.id, ComicStatus::Success);
                            log::debug!("MOBI conversion successful");
                        }
                        Err(e) => {
                            log::error!("MOBI conversion failed: {e}");
                            send_comic_update(
                                &status.event_tx,
                                status.id,
                                ComicStatus::Failed { error: e },
                            );
                        }
                    }
                }
            }
        }

        pending.retain(|s| s.is_some());

        thread::sleep(Duration::from_millis(100));
    }
}

// Helper functions to reduce boilerplate when sending events
fn send_progress(tx: &mpsc::Sender<Event>, event: ProgressEvent) {
    tx.send(Event::Progress(event)).ok();
}

fn error(tx: &mpsc::Sender<Event>, id: usize, error: anyhow::Error) {
    send_comic_update(tx, id, ComicStatus::Failed { error });
}

fn send_comic_update(tx: &mpsc::Sender<Event>, id: usize, status: ComicStatus) {
    send_progress(tx, ProgressEvent::ComicUpdate { id, status });
}

fn send_register_comic(tx: &mpsc::Sender<Event>, id: usize, file_name: String) {
    send_progress(tx, ProgressEvent::RegisterComic { id, file_name });
}

fn send_processing_complete(tx: &mpsc::Sender<Event>) {
    send_progress(tx, ProgressEvent::ProcessingComplete);
}
