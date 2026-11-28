//! Файл: venues/bsc_dex/directory/store_factory.rs
//! Что делает: пулы, найденные поиском по фабрикам на своём узле (решение 121, C3; pools/factory_scan.rs; поле Snap.factory),
//! — в SQLite (storage/venue_state.rs) и обратно при старте подписки (ревью арбитра 28.11.2026, Б4). Без этого площадка,
//! у которой справочник индексатора пуст (живой бот 28.11.2026, base_dex: исчерпан месячный лимит ключа, код поставщика
//! 10006), после каждого перезапуска стартовала с нулём пулов и ждала конца поиска по фабрикам. Восстановленные строки
//! проходят ту же стартовую проверку на своём узле, что строки индексатора (pools/index.rs begin → take), и тот же
//! повтор (index_watch.rs); фабрика пары не удаляет, поэтому сохранённое не стирается. Ключ — chainId сети площадки
//! (dex.evm_chain_id, Ctx::chain_id) и адрес монеты: смена сети — другие строки. Поиск по фабрикам выключен
//! (index.factory_scan не «да») — строки не восстанавливаются (справочник фабрик при выключении очищается,
//! factory_scan.rs off). chainId не задан — не сохраняется и не восстанавливается.
//! Кто вызывает: pools/factory_scan.rs (save — после прохода, монеты с новыми пулами), directory/refresh.rs (restore —
//! до стартовой проверки).
//! От чего зависит: ctx.rs, storage/venue_state.rs, directory/gt_parse.rs (IndexedPool), directory/mod.rs (Snap).
//! Настройки: index.factory_scan, dex.evm_chain_id [bsc_dex] (читаются).

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::Ctx;
use super::gt_parse::IndexedPool;

/// Вид записи (id — «chainId:адрес монеты»).
const FACTORY: &str = "factory_pools";

/// Ошибка SQLite. Принимает: текст. Возвращает: GatewayError Internal.
fn internal(ctx: &Ctx, e: impl std::fmt::Display) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::Internal,
        format!("пулы поиска по фабрикам в SQLite: {e}"),
    )
}

/// Сохраняет найденные фабриками пулы монет (весь список монеты, заменяет прежний). Принимает: окружение, [(монета,
/// пулы)]. chainId не задан — ничего. Ошибки: Internal (база, сериализация).
pub async fn save(ctx: &Ctx, rows: Vec<(Address, Vec<IndexedPool>)>) -> GatewayResult<()> {
    let Some(chain) = ctx.chain_id() else {
        return Ok(());
    };
    if rows.is_empty() {
        return Ok(());
    }
    let mut out = Vec::with_capacity(rows.len());
    for (coin, list) in rows {
        let body = serde_json::to_string(&list).map_err(|e| internal(ctx, e))?;
        out.push((format!("{chain}:{coin:#x}"), body));
    }
    venue_state::save_many(ctx.journal.db(), ctx.place.id, FACTORY, out)
        .await
        .map_err(|e| internal(ctx, e))
}

/// Сохранённые пулы сети по монете. Повреждённая строка пропускается с записью в журнал. Ошибки: Internal.
async fn load(ctx: &Ctx, chain: u64) -> GatewayResult<Vec<(Address, Vec<IndexedPool>)>> {
    let rows = venue_state::list(
        ctx.journal.db(),
        ctx.place.id,
        FACTORY,
        &format!("{chain}:"),
    )
    .await
    .map_err(|e| internal(ctx, e))?;
    let mut out = Vec::new();
    for (id, body) in rows {
        let coin = id
            .rsplit(':')
            .next()
            .and_then(|a| a.parse::<Address>().ok());
        match (coin, serde_json::from_str::<Vec<IndexedPool>>(&body)) {
            (Some(c), Ok(list)) => out.push((c, list)),
            (_, Err(e)) => {
                let msg = format!(
                    "Пулы поиска по фабрикам в SQLite: запись {id} повреждена ({e}) — монета найдётся следующим проходом"
                );
                ctx.log(Level::Warn, "dex.index", msg, None).await;
            }
            (None, Ok(_)) => {}
        }
    }
    Ok(out)
}

/// Восстанавливает сохранённые пулы монет подписки в справочник фабрик (Snap.factory) — до стартовой проверки на узле.
/// Принимает: окружение, монеты подписки (с промежуточными). Поиск выключен или chainId не задан — ничего.
/// Возвращает: сколько пулов восстановлено. Ошибки: Internal.
pub async fn restore(ctx: &Ctx, coins: &HashMap<Address, String>) -> GatewayResult<usize> {
    if ctx.flag("index.factory_scan").ok() != Some(true) {
        return Ok(0);
    }
    let Some(chain) = ctx.chain_id() else {
        return Ok(0);
    };
    let rows = load(ctx, chain).await?;
    Ok(ctx.index.with(|s| {
        let mut n = 0;
        for (coin, list) in rows.into_iter().filter(|(c, _)| coins.contains_key(c)) {
            let have = s.factory.entry(coin).or_default();
            for p in list {
                if !have.contains(&p) {
                    have.push(p);
                    n += 1;
                }
            }
        }
        n
    }))
}
