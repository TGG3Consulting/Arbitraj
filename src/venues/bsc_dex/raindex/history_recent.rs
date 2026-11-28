//! Свежие Add/Remove продвигаются независимо от долгого прохода от deployment.
//! Диапазон, токены и курсор полной истории остаются независимыми от этой выборки.
use super::*;

pub(super) struct Recent {
    pub tx: watch::Sender<Indexed>,
    pub namespace: String,
    loaded: bool,
    expected: Option<String>,
    cursor: Option<Cursor>,
    rows: Arc<Raw>,
}

impl Recent {
    pub fn new(tx: watch::Sender<Indexed>, full: &str) -> Self {
        Self {
            tx, namespace: format!("{full}:recent"), loaded: false,
            expected: None, cursor: None, rows: Arc::default(),
        }
    }
}

impl Worker {
    pub(super) async fn update_recent(&mut self, ctx: &Ctx, head: Cursor, batch: u64) -> GatewayResult<()> {
        let tip = head.0;
        let full = self.cursor.map_or(self.from - 1, |c| c.0);
        if tip <= full.saturating_add(batch) {
            // Полный проход за один диапазон видит все эти же изменения.
            return Ok(());
        }
        let generation = ctx.settings.generation();
        let recent = &mut self.recent;
        if !recent.loaded {
            let saved = history_store::load_recent(ctx, &recent.namespace, self.profile.version)
                .await.map_err(|e| no(ctx, &format!("Raindex: чтение свежих событий: {e}")))?;
            recent.expected = saved.expected;
            if let Some((cursor, rows)) = saved.valid {
                recent.cursor = Some(cursor);
                recent.rows = rows;
            } else {
                recent.cursor = None;
                recent.rows = Arc::default();
            }
            recent.loaded = true;
        }
        let mut reset = false;
        let prior = match recent.cursor {
            Some(c) if c.0 > tip => {
                return Err(no(ctx, "Raindex: узел отстаёт от позиции свежих событий"));
            }
            Some(c) if c == head || (c.0 >= full && sync::hash_at(ctx, c.0).await? == c.1) => c,
            _ => {
                // Только неполная выборка начинает снова у головы; полный курсор сохраняется.
                reset = true;
                recent.tx.send_replace(Indexed::default());
                let n = tip.saturating_sub(batch).max(full);
                (n, sync::hash_at(ctx, n).await?)
            }
        };
        if prior.0 == tip {
            if recent.tx.borrow().cursor != Some(prior) {
                recent.tx.send_replace(Indexed {cursor:Some(prior), rows:recent.rows.clone()});
            }
            return Ok(());
        }
        let end = prior.0.saturating_add(batch).min(tip);
        let (cursor, delta) = sync::range(ctx, self.profile, prior, end, generation).await?;
        let writes: history_orders::Delta = delta.into_iter().map(|(id,row)| (id,Some(row.unwrap_or_default()))).collect();
        if !history_store::commit(ctx.journal.db(), ctx.place.id, &recent.namespace,
                recent.expected.clone(), cursor, reset, writes.clone()).await
            .map_err(|e| no(ctx, &format!("Raindex: запись свежих событий: {e}")))?
        {
            recent.loaded = false;
            return Err(no(ctx, "Raindex: другая задача уже обновила свежие события"));
        }
        if reset {
            recent.rows = Arc::default();
        }
        if !writes.is_empty() {
            let rows = Arc::make_mut(&mut recent.rows);
            for (id,value) in writes {
                if let Some(bytes) = value {
                    rows.insert(id, bytes);
                }
            }
        }
        recent.cursor = Some(cursor);
        recent.expected = Some(serde_json::json!(cursor).to_string());
        recent.tx.send_replace(Indexed { cursor: Some(cursor), rows: recent.rows.clone() });
        Ok(())
    }
}

