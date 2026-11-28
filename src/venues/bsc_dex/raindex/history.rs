//! Фоновая история Add/Remove одной книги Raindex. Подбор читает опубликованные ордера и
//! независимо проверяет текущую ёмкость каждого подходящего ордера от своего модуля.
use super::{
    history_candidates::{Indexed, View},
    history_orders::{self, Cursor},
    history_store,
    profile::{Origin, Source},
    source::Raw,
    *,
};
use crate::storage::journal::Level;
#[path = "history_sync.rs"]
mod sync;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinSet};

type Key = (Profile, u64);
type Orders = Arc<Raw>;
struct Reader {
    rows: watch::Receiver<Indexed>,
    _task: JoinSet<()>,
}
#[derive(Default)]
pub(super) struct Readers(HashMap<Key, Reader>);

fn key(source: &Source) -> Option<Key> {
    if let Origin::Rpc(from) = &source.origin {
        Some((source.profile, *from))
    } else {
        None
    }
}

impl Readers {
    pub(super) fn configure(&mut self, ctx: &Ctx, sources: &[Source]) {
        self.0
            .retain(|id, _| sources.iter().any(|s| key(s) == Some(*id)));
        for id in sources.iter().filter_map(key) {
            self.0.entry(id).or_insert_with(|| {
                let seed = history_orders::seed(id.0, id.1);
                let start = seed
                    .as_ref()
                    .map_or_else(Orders::default, |(_, rows)| rows.clone());
                let (tx, rows) = watch::channel(Indexed {
                    cursor: seed.as_ref().map(|(cursor, _)| *cursor), rows: start,
                });
                let mut task = JoinSet::new();
                task.spawn(run(ctx.clone(), id, seed, tx));
                Reader { rows, _task: task }
            });
        }
    }
    pub(super) fn orders(&self, source: &Source, height: u64) -> Option<View> {
        let id = key(source)?;
        let reader = self.0.get(&id)?;
        Some(View::for_book(id.0, id.1, height, &reader.rows.borrow()))
    }
}

struct Worker {
    profile: Profile,
    from: u64,
    namespace: String,
    seed: Option<(Cursor, Orders)>,
    tx: watch::Sender<Indexed>,
    cursor: Option<Cursor>,
}

async fn run(
    ctx: Ctx,
    (profile, from): Key,
    seed: Option<(Cursor, Orders)>,
    tx: watch::Sender<Indexed>,
) {
    let mut changes = ctx.settings.subscribe();
    let namespace = format!(
        "raindex:v{}:{}:{:#x}:{:#x}:{from}",
        profile.version, profile.chain, profile.book, profile.book_hash
    );
    let mut worker = Worker {
        profile,
        from,
        namespace,
        seed,
        tx,
        cursor: None,
    };
    // При выключенной догрузке известные по прежним сессиям программы тоже доступны для актуального quote2.
    if let Ok(saved) = history_store::load(&ctx, &worker.namespace, profile.version).await
        && let Some((cursor, rows)) = saved.valid
    {
        worker.cursor = Some(cursor);
        worker.tx.send_replace(Indexed { cursor: None, rows });
    }
    let mut last_note = None::<Instant>;
    loop {
        let cadence = ctx.int("bsc.blocks_poll_ms").ok().filter(|v| *v > 0);
        let reporting = ctx
            .int("dex.scan_progress_s")
            .ok()
            .filter(|v| *v > 0)
            .map(Duration::from_secs);
        let delay = if let (Some(ms), Some(interval)) = (cadence, reporting)
            && ctx.flag("dex.history_scan").unwrap_or(false)
            && enabled(&ctx)
        {
            let result = worker.step(&ctx).await;
            if last_note.is_none_or(|t| t.elapsed() >= interval) {
                let count = worker.tx.borrow().rows.len();
                match &result {
                    Ok(Some((n, tip))) => {
                        ctx.log(
                            Level::Info,
                            "dex.scan",
                            format!("Raindex V{}: блок {n} из {tip}, в каталоге {count} ордеров", profile.version),
                            None,
                        )
                        .await
                    }
                    Err(e) => {
                        ctx.log(Level::Warn, "dex.scan", e.message.clone(), None)
                            .await
                    }
                    Ok(None) => {}
                }
                last_note = Some(Instant::now());
            }
            Some(if result.is_err() {
                interval
            } else {
                Duration::from_millis(ms)
            })
        } else {
            None
        };
        match delay {
            // Изменения несвязанных настроек не должны обходить предел запросов узла.
            Some(d) => tokio::time::sleep(d).await,
            None => {
                if matches!(
                    changes.recv().await,
                    Err(tokio::sync::broadcast::error::RecvError::Closed)
                ) {
                    return;
                }
            }
        }
    }
}
