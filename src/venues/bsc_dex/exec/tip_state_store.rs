//! Файл: venues/bsc_dex/exec/tip_state_store.rs
//! Что делает: решение 437 (3), ревью 437 п. 3 — хранение состояния доли чаевых площадки DEX: копия в памяти (по
//! площадке) и SQLite (storage/venue_state.rs: площадка, вид «tip_share») — переживает перезапуск. Изменение —
//! синхронно под замком памяти (change: прочитать-изменить-записать без ожиданий), запись в базу — по одной из очереди
//! и всегда последнего состояния из памяти (persist): параллельные итоги не теряют шаг и счётчики, в базе после
//! перезапуска — последнее.
//! Кто вызывает: exec/tip_state.rs.
//! От чего зависит: storage/venue_state.rs, ctx.rs, core/types (TipShare).
//! Настройки: нет.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use chrono::Utc;

use crate::core::types::TipShare;
use crate::storage::journal::Level;
use crate::storage::venue_state;

use super::super::ctx::Ctx;

/// Вид записи в venue_state и её ключ.
const KIND: &str = "tip_share";
const ID: &str = "state";

/// Площадка DEX → состояние (копия базы).
static ALL: LazyLock<Mutex<HashMap<&'static str, TipShare>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Состояние площадки: из памяти, иначе из базы (нет записи или не разобрана — пустое: доля = минимум). Ошибок нет.
pub async fn get(ctx: &Ctx) -> TipShare {
    if let Some(s) = mem(ctx) {
        return s;
    }
    let body = venue_state::load(ctx.journal.db(), ctx.place.id, KIND, ID).await;
    let st: TipShare = body
        .ok()
        .flatten()
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default();
    let mut all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.entry(ctx.place.id).or_insert(st).clone()
}

/// Копия из памяти. Ошибок нет.
fn mem(ctx: &Ctx) -> Option<TipShare> {
    let all = ALL.lock().unwrap_or_else(|p| p.into_inner());
    all.get(ctx.place.id).cloned()
}

/// Ревью 437, п. 3: изменение состояния площадки — синхронно под замком памяти (прочитать-изменить-записать без
/// ожиданий между ними: параллельный итог не потеряет шаг или счётчик), затем запись в базу (persist). Принимает:
/// окружение, изменение (его итог возвращается). Возвращает: итог изменения. Ошибок нет.
pub async fn change<R>(ctx: &Ctx, f: impl FnOnce(&mut TipShare) -> R) -> R {
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

/// Очередь записи в SQLite: записи идут по одной, каждая берёт ПОСЛЕДНЕЕ состояние из памяти под этой очередью —
/// последняя запись всегда несёт последнее изменение (порядок задач не важен). Ревью 437, п. 3.
static SAVE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Пишет в базу последнее состояние площадки из памяти. Сбой базы — в журнал (после перезапуска доля может быть
/// старой). Ошибок нет.
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
        let msg = format!("Доля чаевых не сохранена в SQLite: {e}");
        ctx.log(Level::Error, "send.tip_share", msg, None).await;
    }
}
