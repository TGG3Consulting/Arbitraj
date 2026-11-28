//! До четырёх источников одновременно. Срок котировки отсчитывается от чтения
//! её блока. Медленная книга не продлевает срок соседней и не снимает её с рынка.
use super::*;
use futures_util::{StreamExt, stream};

pub(super) async fn read(p: &Parts, tx: mpsc::Sender<Batch>) {
    let ctx = &p.ctx;
    let mut changes = ctx.settings.subscribe();
    let mut readers = history::Readers::default();
    loop {
        let generation = ctx.settings.generation();
        let config = || {
            Some((
                Profile::list(ctx)?,
                Duration::from_millis(ctx.int("raindex.max_age_ms").ok()?.max(1)),
                p.tokens.usdt(ctx).ok()?,
            ))
        };
        let active = enabled(ctx).then(config).flatten();
        readers.configure(
            ctx,
            active
                .as_ref()
                .map_or(&[], |(sources, _, _)| sources.as_slice()),
        );
        let Some((sources, ttl, cash)) = active else {
            if matches!(
                changes.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {
                return;
            }
            continue;
        };
        let round = Instant::now();
        let mut pinned = None;
        for group in sources.chunks(4) {
            if generation != ctx.settings.generation() {
                break;
            }
            // Переиспользуем уже прочитанный блок, когда следующая группа ещё
            // располагает своим обычным бюджетом чтения в пределах его TTL.
            if pinned.is_none_or(|(at, _): (Instant, white_state::Head)| at.elapsed() >= ttl / 2) {
                let at = Instant::now();
                pinned = white_state::head(ctx, "latest")
                    .await
                    .ok()
                    .map(|head| (at, head));
            }
            let started = pinned.map_or_else(Instant::now, |(at, _)| at);
            let mut completed = stream::iter(group.iter().cloned())
                .map(|src| {
                    let current = pinned.map(|(_, head)| {
                        let catalog = readers.orders(&src, head.number.to::<u64>());
                        (head, catalog)
                    });
                    async move {
                        let rows = match current {
                            Some((head, catalog)) => {
                                tokio::time::timeout(ttl / 2, src.read(p, head, cash, catalog))
                                    .await
                                    .ok()
                                    .flatten()
                            }
                            None => None,
                        };
                        (src.profile.book, rows)
                    }
                })
                .buffer_unordered(4)
                .ready_chunks(4);
            while let Some(books) = completed.next().await {
                if tx
                    .send(Batch {
                        generation,
                        started,
                        ttl,
                        cash,
                        books,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        let delay = (ttl / 2).max(Duration::from_millis(1));
        // Одна прежняя пауза на полный обход независимо от числа книг.
        let delay = if sources.len() > 4 {
            delay.saturating_sub(round.elapsed())
        } else {
            delay
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            v = changes.recv() => if matches!(v, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return; }
        }
    }
}
