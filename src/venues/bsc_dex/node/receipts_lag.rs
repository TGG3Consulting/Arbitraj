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
}

/// Отставание ленты и блок, на котором она стоит.
#[derive(Default)]
pub struct Lag {
    stuck: Option<Stuck>,
    /// Отставание, о котором уже записано (0 — не отстаём).
    told: u64,
    /// Стоит отметка «данные пулов отстают» (решение 72).
    marked: bool,
}

impl Lag {
    /// Блок `n` не пришёл (причина `why`). Ошибок нет.
    pub fn failed(&mut self, n: u64, why: &str) {
        match &mut self.stuck {
            Some(s) if s.n == n => {
                s.tries += 1;
                s.why = why.to_string();
            }
            _ => {
                self.stuck = Some(Stuck {
                    n,
                    since: Instant::now(),
                    tries: 1,
                    why: why.to_string(),
                })
            }
        }
    }

    /// Блок `n` разобран: если конвейер стоял на нём — больше не стоит. Ошибок нет.
    pub fn passed(&mut self, n: u64) {
        if self.stuck.as_ref().is_some_and(|s| s.n <= n) {
            self.stuck = None;
        }
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
                    "{SOURCE}: отставание {behind} блоков — не больше bsc.receipts_max_lag_blocks, котировки и сделки снова разрешены"
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
