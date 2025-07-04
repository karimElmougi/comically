use crate::{
    comic::{Comic, ComicConfig, ComicStage, ComicStatus, ProgressEvent},
    comic_archive, epub_builder, image_processor, mobi_converter, Event,
};
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
    prefix: Option<String>,
    event_tx: mpsc::Sender<Event>,
    kindlegen_tx: mpsc::Sender<Comic>,
) {
    log::info!("processing with config: {:?}", config);
    log::info!("processing {} files", files.len());

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
                id,
                file.clone(),
                prefix.as_deref(),
                title,
                config,
                event_tx.clone(),
            ) {
                Ok(comic) => Some(comic),
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
        .filter_map(|mut comic| {
            let images = comic.with_try(|comic| {
                let archive_iter = comic_archive::unarchive_comic_iter(&comic.input)?;
                let num_images = archive_iter.num_images();
                let start = comic.image_processing_start(num_images);
                let images = image_processor::process_archive_images(
                    archive_iter,
                    config,
                    comic.processed_dir(),
                    comic.id,
                    &comic.tx,
                )?;
                comic.image_processing_complete(start.elapsed());
                Ok(images)
            })?;

            log::info!("Processed {} images for {}", images.len(), comic.title);

            comic.processed_files = images;

            comic.with_try(|comic| {
                let start = comic.update_status(ComicStage::Epub, 50.0);
                epub_builder::build_epub(comic)?;
                comic.stage_completed(ComicStage::Epub, start.elapsed());
                Ok(())
            })?;
            Some(comic)
        })
        .for_each(|comic| {
            kindlegen_tx.send(comic).unwrap();
        });
}

pub fn poll_kindlegen(tx: mpsc::Receiver<Comic>) {
    struct KindleGenStatus {
        comic: Comic,
        spawned: mobi_converter::SpawnedKindleGen,
        start: Instant,
    }

    let mut pending = Vec::<Option<KindleGenStatus>>::new();

    'outer: loop {
        loop {
            let result = tx.try_recv();

            match result {
                Ok(mut comic) => {
                    let result = comic.with_try(|comic| {
                        let start = comic.update_status(ComicStage::Mobi, 75.0);
                        let spawned = mobi_converter::create_mobi(comic)?;
                        Ok((spawned, start))
                    });
                    if let Some((spawned, start)) = result {
                        pending.push(Some(KindleGenStatus {
                            comic,
                            spawned,
                            start,
                        }));
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
                if let Some(mut status) = s.take() {
                    let _ = status.comic.with_try(|comic| {
                        status.spawned.wait()?;
                        comic.stage_completed(ComicStage::Mobi, status.start.elapsed());
                        comic.success();
                        Ok(())
                    });
                }
            }
        }

        pending.retain(|s| s.is_some());

        thread::sleep(Duration::from_millis(100));
    }
}
