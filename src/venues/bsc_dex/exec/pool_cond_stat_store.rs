//! Файл: venues/bsc_dex/exec/pool_cond_stat_store.rs
//! Что делает: решение 439 — хранение статистики условия пулов площадки DEX тем же способом, что доля чаевых
//! (exec/tip_state_store.rs): копия в памяти по площадке и SQLite (storage/venue_state.rs: площадка, вид
//! «pool_cond_stat») — переживает перезапуск. Изменение — синхронно под замком памяти (change), запись в базу — по
//! одной из очереди и всегда последнего состояния из памяти (persist): параллельные итоги не теряют счётчики.
//! Кто вызывает: exec/pool_cond_stat.rs.
//! От чего зависит: storage/venue_state.rs, ctx.rs, core/types (PoolCondStat).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;

use crate::core::types::PoolCondStat;
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::Ctx;

/// Вид записи в venue_state и её ключ.
const KIND: &str = "pool_cond_stat";
const ID: &str = "state";

/// Площадка DEX → статистика (копия базы).
static ALL: LazyLock<Mutex<HashMap<&'static str, PoolCondStat>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Статистика площадки: из памяти, иначе из базы (нет записи или не разобрана — пустая). Ошибок нет.
pub async fn get(ctx: &Ctx) -> PoolCondStat {
    if let Some(s) = mem(ctx) {
        return s;
    }
    let body = venue_state::load(ctx.journal.db(), ctx.place.id, KIND, ID).await;
    let st: PoolCondStat = body
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let mut all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.entry(ctx.place.id).or_insert(st).clone()
}

/// Копия из памяти. Ошибок нет.
fn mem(ctx: &Ctx) -> Option<PoolCondStat> {
    let all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.get(ctx.place.id).cloned()
}

/// Изменение статистики площадки — синхронно под замком памяти, затем запись в базу (persist). Принимает: окружение,
/// изменение (его итог возвращается). Возвращает: итог изменения. Ошибок нет.
pub async fn change<R>(ctx: &Ctx, f: impl FnOnce(&mut PoolCondStat) -> R) -> R {
    // Первое обращение — загрузка из базы в память (get); дальше всё — в памяти.
    get(ctx).await;
    let out = {
        let mut all = ALL.lock().unwrap_or_else(|p| p.into_inner());
        let st = all.entry(ctx.place.id).or_default();
        let r = f(st);
        st.updated_at = Some(Utc::now());
        r
    };
    persist(ctx).await;
    out
}

/// Очередь записи в SQLite: записи идут по одной, каждая берёт ПОСЛЕДНЕЕ состояние из памяти под этой очередью.
static SAVE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Пишет в базу последнее состояние площадки из памяти. Сбой базы — в журнал send.pool_cond_stat (после перезапуска
/// статистика может быть старой). Ошибок нет.
async fn persist(ctx: &Ctx) {
    let _turn = SAVE.lock().await;
    let Some(st) = mem(ctx) else { return };
    let saved = match serde_json::to_string(&st) {
        Ok(body) => venue_state::save(ctx.journal.db(), ctx.place.id, KIND, ID, body)
            .await
            .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = saved {
        let msg = format!("Статистика условия пулов не сохранена в SQLite: {e}");
        ctx.log(Level::Error, "send.pool_cond_stat", msg, None).await;
    }
}
