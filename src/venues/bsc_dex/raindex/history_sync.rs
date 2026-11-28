//! Каноническая привязка сохранённого снимка и одного диапазона событий Raindex.
use super::*;
use crate::venues::bsc_dex::pools::scan;

pub(super) async fn hash_at(ctx: &Ctx, height: u64) -> GatewayResult<B256> {
    Ok(white_state::head(ctx, &format!("{height:#x}")).await?.hash)
}

pub(super) async fn range(
    ctx: &Ctx, profile: Profile, prior: Cursor, end: u64, generation: u64,
) -> GatewayResult<(Cursor, history_orders::Delta)> {
    let (last, hash) = prior;
    let head = hash_at(ctx, end).await?;
    let events = history_orders::topics(profile.version)
        .ok_or_else(|| no(ctx, "Raindex: версия событий книги неизвестна"))?;
    let topics = serde_json::json!([events]);
    let book = profile.book;
    let mut logs = scan::one(ctx, book, &topics, last + 1, end, "Raindex").await?;
    let delta = history_orders::changes(profile, last + 1, end, &mut logs)
        .ok_or_else(|| no(ctx, "Raindex: неполные или несовместимые события книги"))?;
    let mut checked = None;
    for log in &logs {
        let height = log.block_number.unwrap_or_default();
        if checked.is_none_or(|(n, _)| n != height) {
            checked = Some((
                height,
                if height == end {
                    head
                } else {
                    hash_at(ctx, height).await?
                },
            ));
        }
        if log.block_hash != checked.map(|(_, h)| h) {
            return Err(no(ctx, "Raindex: событие с другой ветви"));
        }
    }
    if hash_at(ctx, last).await? != hash
        || hash_at(ctx, end).await? != head
        || ctx.settings.generation() != generation
    {
        return Err(no(
            ctx,
            "Raindex: изменился канон или настройка при чтении диапазона",
        ));
    }
    Ok(((end, head), delta))
}

impl Worker {
    async fn restore(&mut self, ctx: &Ctx, tip: u64) -> GatewayResult<()> {
        let saved = history_store::load(ctx, &self.namespace, self.profile.version)
            .await
            .map_err(|e| no(ctx, &format!("Raindex: чтение позиции: {e}")))?;
        let mut replaced = None;
        if let Some((cursor, orders)) = saved.valid {
            if tip < cursor.0 {
                self.tx.send_replace(Indexed { cursor: None, rows: orders });
                return Err(no(ctx, "Raindex: узел ещё не достиг сохранённого блока"));
            }
            if hash_at(ctx, cursor.0).await? == cursor.1 {
                self.tx.send_replace(Indexed { cursor: Some(cursor), rows: orders });
                self.cursor = Some(cursor);
                return Ok(());
            }
            replaced = Some(cursor);
        }
        let (cursor, orders) = if let Some((c, rows)) = &self.seed
            && tip >= c.0
            && hash_at(ctx, c.0).await? == c.1
        {
            (*c, rows.clone())
        } else {
            if tip < self.from {
                return Err(no(ctx, "Raindex: книга ещё не размещена"));
            }
            let height = self.from - 1;
            ((height, hash_at(ctx, height).await?), Orders::default())
        };
        if let Some((height, old_hash)) = replaced
            && hash_at(ctx, height).await? == old_hash
        {
            return Err(no(
                ctx,
                "Raindex: сохранённый блок вновь каноничен, перечитаем позицию",
            ));
        }
        if hash_at(ctx, cursor.0).await? != cursor.1 {
            return Err(no(
                ctx,
                "Raindex: исходный блок изменился перед сохранением",
            ));
        }
        let delta = orders.iter().map(|(&h, b)| (h, Some(b.clone()))).collect();
        let ok = history_store::commit(
            ctx.journal.db(),
            ctx.place.id,
            &self.namespace,
            saved.expected,
            cursor,
            true,
            delta,
        )
        .await
        .map_err(|e| no(ctx, &format!("Raindex: начало истории: {e}")))?;
        if !ok {
            return Err(no(ctx, "Raindex: другой читатель уже обновил позицию"));
        }
        self.cursor = Some(cursor);
        self.tx.send_replace(Indexed { cursor: Some(cursor), rows: orders });
        Ok(())
    }

    pub(super) async fn step(&mut self, ctx: &Ctx) -> GatewayResult<Option<(u64, u64)>> {
        let head = white_state::head(ctx, "latest").await?;
        let tip = (head.number.to::<u64>(), head.hash);
        let batch = scan::batch_size(ctx)?;
        // Ошибка при чтении старого блока не должна остановить наблюдение за новыми ордерами.
        let live = self.update_recent(ctx, tip, batch).await;
        let full = self.advance(ctx, tip, batch).await;
        live?;
        full
    }

    async fn advance(&mut self, ctx: &Ctx, head: Cursor, batch: u64) -> GatewayResult<Option<(u64,u64)>> {
        let tip = head.0;
        let generation = ctx.settings.generation();
        if self.cursor.is_none() {
            self.restore(ctx, tip).await?;
        }
        let (last, hash) = self
            .cursor
            .ok_or_else(|| no(ctx, "Raindex: отсутствует позиция"))?;
        if tip < last {
            return Err(no(ctx, "Raindex: узел отстаёт от позиции"));
        }
        let canonical = if tip == last { head.1 } else { hash_at(ctx, last).await? };
        if canonical != hash {
            self.cursor = None;
            self.tx.send_modify(|s| s.cursor = None);
            return Err(no(
                ctx,
                "Raindex: реорганизация, переопределяем каноническую позицию",
            ));
        }
        if tip == last {
            if self.tx.borrow().cursor.is_none() {
                self.tx.send_modify(|s| s.cursor = Some((last, hash)));
            }
            return Ok(None);
        }
        let end = tip.min(last.saturating_add(batch));
        let (cursor, delta) = range(ctx, self.profile, (last, hash), end, generation).await?;
        let expected = Some(serde_json::json!((last, hash)).to_string());
        if !history_store::commit(
            ctx.journal.db(),
            ctx.place.id,
            &self.namespace,
            expected,
            cursor,
            false,
            delta.clone(),
        )
        .await
        .map_err(|e| no(ctx, &format!("Raindex: сохранение позиции: {e}")))?
        {
            self.cursor = None;
            return Err(no(ctx, "Raindex: другой читатель уже сохранил позицию"));
        }
        self.cursor = Some(cursor);
        self.tx.send_modify(|snapshot| {
            snapshot.cursor = Some(cursor);
            if !delta.is_empty() {
                let raw = Arc::make_mut(&mut snapshot.rows);
                for (id, value) in delta {
                    if let Some(bytes) = value {
                        raw.insert(id, bytes);
                    } else {
                        raw.remove(&id);
                    }
                }
            }
        });
        Ok(Some((end, tip)))
    }
}
