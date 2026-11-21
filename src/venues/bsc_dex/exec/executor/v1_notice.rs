//! Файл: venues/bsc_dex/exec/executor/v1_notice.rs
//! Что делает: ревью 272 (Б5) — уведомление «у кошелька X исполнитель v1 — нативный BNB через форки не торгуется»
//! (событие wallet.executor_v1: журнал dex.executor и Telegram, если событие включено — notify/catalog.rs). Один раз на
//! пару (кошелёк, адрес исполнителя) за время работы процесса — не спамом: сверка идёт при каждой сделке через
//! исполнитель и при каждой «Проверить подключение», а уведомление — только первое (после перезапуска — снова одно).
//! Кто вызывает: venue.rs (check_executor — «Проверить подключение», после развёртывания и «Записать после проверки»),
//! exec/executor/trade.rs (после сверки onchain.rs::ensure).
//! От чего зависит: exec/executor/code.rs (Version), send/stats.rs (событие ядру → notify/dispatch.rs), ctx.rs.
//! Настройки: notify.event.* события wallet.executor_v1 (экран «Уведомления»); своих нет.

use std::collections::HashSet;
use std::sync::Mutex;

use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::super::ctx::Ctx;
use super::super::super::send::stats::Stats;
use super::code::Version;

/// (площадка DEX, id кошелька, адрес исполнителя 0x…), о которых уже сообщено (решение 435: исполнитель кошелька в каждой
/// сети свой).
type Said = (&'static str, String, String);
static SENT: Mutex<Option<HashSet<Said>>> = Mutex::new(None);

/// Сообщает, если сверенная версия исполнителя кошелька окружения — v1 и об этой паре ещё не сообщали. Принимает:
/// окружение, счётчики отправки (канал событий ядру), версию. Ошибок нет (кошелька или адреса нет — ничего).
pub async fn once(ctx: &Ctx, stats: &Stats, v: Version) {
    if v != Version::V1 {
        return;
    }
    let Ok(w) = ctx.wallet() else { return };
    let Some(x) = w.executor.clone() else { return };
    let fresh = SENT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert((ctx.place.id, w.id.clone(), x.to_ascii_lowercase()));
    if !fresh {
        return;
    }
    let text = format!(
        "У кошелька «{}» исполнитель {x} версии v1 — нативный BNB через форки и виды «только исполнитель» не \
         торгуется (ERC-20 через исполнитель — как обычно). Чтобы торговать BNB — переразверните исполнитель \
         (docs/notes/executor-v2-redeploy.md)",
        w.name
    );
    ctx.log(Level::Warn, "dex.executor", text.clone(), None)
        .await;
    stats.emit(MarketEvent::Notice {
        venue: ctx.venue(),
        code: "wallet.executor_v1".into(),
        text,
    });
}
