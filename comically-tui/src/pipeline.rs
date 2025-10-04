use crate::progress::{ComicStage, ComicStatus, ProgressEvent};
use crate::Event;
use anyhow::Context;
use comically::{Comic, ComicConfig, OutputFormat};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::{
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn process_files(
    files: Vec<PathBuf>,
    config: ComicConfig,
    output_dir: PathBuf,
    event_tx: mpsc::Sender<Event>,
) {
    log::info!("processing with config: {:?}", config);
    log::info!("processing {} files", files.len());

    let (kindlegen_tx, kindlegen_rx) = mpsc::channel::<(usize, Comic, mpsc::Sender<Event>)>();

    if config.output_format == OutputFormat::Mobi {
        let event_tx = event_tx.clone();
        thread::spawn(move || {
            poll_kindlegen(kindlegen_rx);
            // after all the comics have finished conversion to mobi, send the complete event
            event_tx
                .send(Event::Progress(ProgressEvent::ProcessingComplete))
                .unwrap();
        });
    }

    let comics: Vec<_> = files
        .into_iter()
        .enumerate()
        .filter_map(|(id, file)| {
            let title = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            event_tx
                .send(Event::Progress(ProgressEvent::RegisterComic {
                    id,
                    file_name: title.clone(),
                }))
                .unwrap();

            match Comic::new(
                file.clone(),
                output_dir.clone(),
                title,
                config.clone(),
            ) {
                Ok(comic) => Some((id, comic)),
                Err(e) => {
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Failed { error: e },
                        }))
                        .unwrap();
                    None
                }
            }
        })
        .collect();

    comics
        .into_iter()
        .par_bridge()
        .filter_map(|(id, mut comic)| {
            // Process images
            let start = Instant::now();
            let archive_iter = match comically::archive::unarchive_comic_iter(&comic.input) {
                Ok(iter) => iter,
                Err(e) => {
                    log::error!("Error in comic: {} {e}", comic.title);
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Failed { error: e },
                        }))
                        .ok();
                    return None;
                }
            };
            
            let num_images = archive_iter.num_images();
            event_tx
                .send(Event::Progress(ProgressEvent::ComicUpdate {
                    id,
                    status: ComicStatus::ImageProcessingStart {
                        total_images: num_images,
                        start,
                    },
                }))
                .ok();
            
            let images = match comically::image::process_archive_images(
                archive_iter,
                &config,
                comic.processed_dir(),
            ) {
                Ok(imgs) => imgs,
                Err(e) => {
                    log::error!("Error processing images for {}: {e}", comic.title);
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Failed { error: e },
                        }))
                        .ok();
                    return None;
                }
            };
            
            event_tx
                .send(Event::Progress(ProgressEvent::ComicUpdate {
                    id,
                    status: ComicStatus::ImageProcessingComplete {
                        duration: start.elapsed(),
                    },
                }))
                .ok();

            log::info!("Processed {} images for {}", images.len(), comic.title);

            comic.processed_files = images;

            // Build output format
            let build_start = Instant::now();
            event_tx
                .send(Event::Progress(ProgressEvent::ComicUpdate {
                    id,
                    status: ComicStatus::Progress {
                        stage: ComicStage::Package,
                        progress: 75.0,
                        start: build_start,
                    },
                }))
                .ok();
            
            let build_result = match config.output_format {
                OutputFormat::Cbz => comically::cbz::build(&comic),
                OutputFormat::Epub => {
                    comically::epub::build(&comic).and_then(|_| {
                        // Move EPUB to final destination
                        let output_path = comic.output_path();
                        std::fs::rename(comic.epub_file(), &output_path).with_context(|| {
                            format!("Failed to move EPUB to output: {:?}", output_path)
                        })
                    })
                }
                OutputFormat::Mobi => {
                    match comically::epub::build(&comic) {
                        Ok(_) => {
                            kindlegen_tx.send((id, comic, event_tx.clone())).ok();
                            return Some(());
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            
            match build_result {
                Ok(_) => {
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::StageCompleted {
                                stage: ComicStage::Package,
                                duration: build_start.elapsed(),
                            },
                        }))
                        .ok();
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Success,
                        }))
                        .ok();
                    Some(())
                }
                Err(e) => {
                    log::error!("Error building output for {}: {e}", comic.title);
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Failed { error: e },
                        }))
                        .ok();
                    None
                }
            }
        })
        .for_each(|_| {});

    match config.output_format {
        OutputFormat::Epub | OutputFormat::Cbz => {
            event_tx
                .send(Event::Progress(ProgressEvent::ProcessingComplete))
                .unwrap();
        }
        _ => {}
    }
}

pub fn poll_kindlegen(tx: mpsc::Receiver<(usize, Comic, mpsc::Sender<Event>)>) {
    struct KindleGenStatus {
        id: usize,
        comic: Comic,
        spawned: comically::mobi::SpawnedKindleGen,
        start: Instant,
        event_tx: mpsc::Sender<Event>,
    }

    let mut pending = Vec::<Option<KindleGenStatus>>::new();

    'outer: loop {
        loop {
            let result = tx.try_recv();

            match result {
                Ok((id, comic, event_tx)) => {
                    let start = Instant::now();
                    event_tx
                        .send(Event::Progress(ProgressEvent::ComicUpdate {
                            id,
                            status: ComicStatus::Progress {
                                stage: ComicStage::Convert,
                                progress: 75.0,
                                start,
                            },
                        }))
                        .ok();
                    
                    match comically::mobi::create(&comic) {
                        Ok(spawned) => {
                            pending.push(Some(KindleGenStatus {
                                id,
                                comic,
                                spawned,
                                start,
                                event_tx,
                            }));
                        }
                        Err(e) => {
                            log::error!("Error creating MOBI for {}: {e}", comic.title);
                            event_tx
                                .send(Event::Progress(ProgressEvent::ComicUpdate {
                                    id,
                                    status: ComicStatus::Failed { error: e },
                                }))
                                .ok();
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
                    log::debug!("KindleGen process completed for: {}", status.comic.title);
                    match status.spawned.wait() {
                        Ok(_) => {
                            status.event_tx
                                .send(Event::Progress(ProgressEvent::ComicUpdate {
                                    id: status.id,
                                    status: ComicStatus::StageCompleted {
                                        stage: ComicStage::Convert,
                                        duration: status.start.elapsed(),
                                    },
                                }))
                                .ok();
                            status.event_tx
                                .send(Event::Progress(ProgressEvent::ComicUpdate {
                                    id: status.id,
                                    status: ComicStatus::Success,
                                }))
                                .ok();
                            log::debug!("MOBI conversion successful for: {}", status.comic.title);
                        }
                        Err(e) => {
                            log::error!("MOBI conversion failed for {}: {e}", status.comic.title);
                            status.event_tx
                                .send(Event::Progress(ProgressEvent::ComicUpdate {
                                    id: status.id,
                                    status: ComicStatus::Failed { error: e },
                                }))
                                .ok();
                        }
                    }
                }
            }
        }

        pending.retain(|s| s.is_some());

        thread::sleep(Duration::from_millis(100));
    }
}
