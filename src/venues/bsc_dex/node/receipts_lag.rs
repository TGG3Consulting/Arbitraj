//! Файл: venues/bsc_dex/node/receipts_lag.rs
//! Что делает: отставание ленты квитанций (решение 71): на каком блоке и почему конвейер стоит (блок, сколько повторов,
//! сколько секунд, последняя причина — с размером ответа и пределом, receipts_io.rs), — для записи при обрыве по
//! отставанию; повтор этого блока идёт сразу на все узлы (rpc_capped::batch_race). Пока отставание меньше
//! bsc.receipts_max_lag_blocks, пулы не недостоверны: растущее отставание — запись «данные пулов отстают на N блоков»
//! каждый раз, когда оно выросло ещё на окно опережения bsc.receipts_ahead_blocks (окно — естественный шаг: конвейер
//! за один заход нагоняет столько), и одна запись «догнала», когда отставание снова не больше окна.
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: ctx.rs.
//! Настройки: через вызывающего — bsc.receipts_ahead_blocks.

use std::time::Instant;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::receipts::SOURCE;

/// Блок, на котором стоит конвейер.
struct Stuck {
    n: u64,
    since: Instant,
    tries: u32,
    why: String,
    /// «Блока нет нигде» подряд (Fail::Missing) — для одного узла невосполнимо только со второго.
    nowhere: u32,
}

/// Отставание ленты и блок, на котором она стоит.
#[derive(Default)]
pub struct Lag {
    stuck: Option<Stuck>,
    /// Отставание, о котором уже записано (0 — не отстаём).
    told: u64,
    /// Стоит отметка «данные пулов отстают» (решение 72).
    marked: bool,
    /// О незаданной bsc.receipts_stuck_reset_s уже записано в этом застревании.
    no_reset_told: bool,
}

impl Lag {
    /// Блок `n` не пришёл (причина `why`; `missing` — «блока нет нигде», иначе счёт таких подряд обнуляется). Ошибок нет.
    pub fn failed(&mut self, n: u64, why: &str, missing: bool) {
        match &mut self.stuck {
            Some(s) if s.n == n => {
                s.tries += 1;
                s.why = why.to_string();
                s.nowhere = if missing { s.nowhere + 1 } else { 0 };
            }
            _ => {
                self.stuck = Some(Stuck {
                    n,
                    since: Instant::now(),
                    tries: 1,
                    why: why.to_string(),
                    nowhere: missing as u32,
                })
            }
        }
    }

    /// Сколько раз подряд перед этим блок `n` «нет нигде». Ошибок нет.
    pub fn nowhere(&self, n: u64) -> u32 {
        self.stuck
            .as_ref()
            .filter(|s| s.n == n)
            .map_or(0, |s| s.nowhere)
    }

    /// Блок `n` разобран: если конвейер стоял на нём — больше не стоит. Ошибок нет.
    pub fn passed(&mut self, n: u64) {
        if self.stuck.as_ref().is_some_and(|s| s.n <= n) {
            self.stuck = None;
            self.no_reset_told = false;
        }
    }

    /// Блок стоит дольше bsc.receipts_stuck_reset_s (решение 72: тяжёлый блок, больше предела на всех узлах, иначе
    /// держал бы запрет торговли вечно и молча) — причина полного сброса; запись в журнал и событие dex.receipts_stuck
    /// в Telegram с причиной (предел и размер ответа). Настройка не задана — одна запись за застревание, сброса нет.
    /// Принимает: окружение, ленту (канал событий ядру). Возвращает: причину сброса или None. Ошибок нет.
    pub async fn overdue(&mut self, ctx: &Ctx, feed: &super::receipts::Feed) -> Option<String> {
        let s = self.stuck.as_ref()?;
        let limit = match ctx.int("bsc.receipts_stuck_reset_s") {
            Ok(v) => v,
            Err(e) => {
                if !std::mem::replace(&mut self.no_reset_told, true) {
                    let msg = format!(
                        "{SOURCE}: лента стоит на блоке {} — {}; застрявший блок не сбрасывается",
                        s.n, e.message
                    );
                    ctx.log(Level::Warn, "venue.ws", msg, None).await;
                }
                return None;
            }
        };
        if s.since.elapsed().as_secs() < limit {
            return None;
        }
        let why = format!(
            "блок стоит дольше bsc.receipts_stuck_reset_s = {limit} с — полный сброс{}",
            self.stuck_text()
        );
        feed.notice(ctx, "dex.receipts_stuck", format!("{SOURCE}: {why}"))
            .await;
        Some(why)
    }

    /// Блок, на котором стоит конвейер (его повтор — гонкой на все узлы). Ошибок нет.
    pub fn hot(&self) -> Option<u64> {
        self.stuck.as_ref().map(|s| s.n)
    }

    /// Для записи при обрыве: на каком блоке и почему лента стояла. Пусто — не стояла. Ошибок нет.
    pub fn stuck_text(&self) -> String {
        match &self.stuck {
            None => String::new(),
            Some(s) => format!(
                "; лента стояла на блоке {}: повторов {}, {} с, последняя причина — {}",
                s.n,
                s.tries,
                s.since.elapsed().as_secs(),
                s.why
            ),
        }
    }

    /// Отметка «данные пулов отстают» (решение 72): отставание `behind` больше `max_lag` — котировка и сделка запрещены
    /// (`flag` — events_lag.rs), лента догоняет без сброса; снова не больше — отметка снята. Запись — при смене. Ошибок
    /// нет.
    pub async fn mark(
        &mut self,
        ctx: &Ctx,
        flag: &super::events_lag::EventsLag,
        behind: u64,
        max_lag: u64,
    ) {
        let over = behind > max_lag;
        flag.set(if over { behind } else { 0 });
        if over != self.marked {
            self.marked = over;
            let msg = if over {
                format!(
                    "{SOURCE}: отставание {behind} блоков больше bsc.receipts_max_lag_blocks = {max_lag} — котировки и сделки запрещены, лента догоняет без сброса (состояние пулов сохранено){}",
                    self.stuck_text()
                )
            } else {
                format!(
                    "{SOURCE}: отставание {behind} блоков ≤ bsc.receipts_max_lag_blocks = {max_lag} — отметка снята, котировки и сделки снова разрешены"
                )
            };
            let level = if over { Level::Warn } else { Level::Info };
            ctx.log(level, "venue.ws", msg, None).await;
        }
    }

    /// Отставание `lag` блоков при окне `ahead`: выросло ещё на окно — запись «отстают на N блоков»; снова не больше
    /// окна после записи — «догнала». Ошибок нет.
    pub async fn report(&mut self, ctx: &Ctx, lag: u64, ahead: u64) {
        let step = ahead.max(1);
        if lag > step && lag >= self.told + step {
            self.told = lag;
            let at = self
                .stuck
                .as_ref()
                .map(|s| format!(" (стоит на блоке {}: {})", s.n, s.why))
                .unwrap_or_default();
            let msg = format!(
                "{SOURCE}: данные пулов отстают на {lag} блоков{at} — пулы пока достоверны, предел bsc.receipts_max_lag_blocks"
            );
            ctx.log(Level::Warn, "venue.ws", msg, None).await;
        } else if lag <= step && self.told > 0 {
            self.told = 0;
            let msg = format!("{SOURCE}: лента догнала сеть, отставание {lag} блоков");
            ctx.log(Level::Info, "venue.ws", msg, None).await;
        }
    }
}
