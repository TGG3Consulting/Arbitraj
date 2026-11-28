//! До четырёх источников одновременно. Срок котировки отсчитывается от чтения
//! её блока. Медленная книга не продлевает срок соседней и не снимает её с рынка.
use super::*;
use futures_util::{StreamExt, stream};

type Updates = Vec<(Address, Option<Vec<Live>>)>;
pub(super) struct Batch {
    generation: u64,
    started: Instant,
    ttl: Duration,
    cash: Address,
    books: Updates,
}

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

struct Books<T>(BTreeMap<Address, (Instant, Vec<T>)>);

impl<T> Books<T> {
    fn replace(&mut self, book: Address, at: Instant, rows: Vec<T>) {
        if rows.is_empty() {
            self.0.remove(&book);
        } else {
            self.0.insert(book, (at, rows));
        }
    }
    fn expire(&mut self, now: Instant, ttl: Duration) {
        self.0
            .retain(|_, (at, _)| now.saturating_duration_since(*at) < ttl);
    }
    fn oldest(&self) -> Option<Instant> {
        self.0.values().map(|(at, _)| *at).min()
    }
}

pub(super) async fn publish(
    p: &Parts,
    mut rx: mpsc::Receiver<Batch>,
    out: mpsc::Sender<MarketEvent>,
) {
    let ctx = &p.ctx;
    let mut changes = ctx.settings.subscribe();
    let mut books = Books::<Arc<Live>>(BTreeMap::new());
    let mut generation = ctx.settings.generation();
    let mut cash = Address::ZERO;
    let mut ttl = Duration::ZERO;
    loop {
        let next_expiry = books.oldest().and_then(|at| at.checked_add(ttl));
        let expire = async {
            match next_expiry {
                Some(at) => tokio::time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        let result = tokio::select! {
            value = rx.recv() => match value {Some(b) => Some(b), None => return},
            _ = expire => None,
            v = changes.recv() => {
                if matches!(v, Err(tokio::sync::broadcast::error::RecvError::Closed)) { return; }
                None
            }
        };
        let current = ctx.settings.generation();
        if generation != current {
            books.0.clear();
            generation = current;
            cash = Address::ZERO;
        }
        if let Some(b) = result
            && current == b.generation
        {
            ttl = b.ttl;
            cash = b.cash;
            for (book, rows) in b.books {
                books.replace(
                    book,
                    b.started,
                    rows.into_iter().flatten().map(Arc::new).collect(),
                );
            }
        }
        books.expire(Instant::now(), ttl);
        let oldest = books.oldest();
        let mut markets = BTreeMap::new();
        let mut rows = Vec::new();
        for r in books.0.values().flat_map(|(_, rows)| rows) {
            let (input, output) = r.leg.pair();
            let buy = input == cash;
            let side = markets
                .entry(if buy { output } else { input })
                .or_insert((false, false));
            if buy {
                side.0 = true;
            } else {
                side.1 = true;
            }
            rows.push(r.clone());
        }
        let had = {
            let mut state = p.raindex.state.lock().unwrap_or_else(|e| e.into_inner());
            let had = state.rows.is_some();
            state.rows = oldest.map(|at| (current, at, Arc::new(rows)));
            had
        };
        if oldest.is_none() && !had {
            continue;
        }
        let expires_ms = oldest.map_or(0, |at| {
            chrono::Utc::now().timestamp_millis().saturating_add(
                i64::try_from(ttl.saturating_sub(at.elapsed()).as_millis()).unwrap_or(i64::MAX),
            )
        });
        let event = MarketEvent::RfqMarkets {
            source: SourceId::new(LABEL),
            venue: ctx.venue(),
            quote: if oldest.is_some() {
                cash
            } else {
                Address::ZERO
            },
            markets,
            expires_ms,
        };
        if out.send(event).await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
#[path = "book_refresh_tests.rs"]
mod tests;
