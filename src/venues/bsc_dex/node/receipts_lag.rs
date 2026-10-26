//! Файл: venues/bsc_dex/node/receipts_lag.rs
//! Что делает: отставание ленты квитанций (решения 71, 72, 77): на каком блоке и почему конвейер стоит (блок, сколько
//! повторов, сколько «жёстких» отказов «ответ больше предела», секунды, последняя причина — с размером ответа и
//! пределом), — для записи и для двух условий восстановления «пропуск + пересинхрон» (решение 77):
//! - один блок дал bsc.receipts_block_max_tries «жёстких» отказов подряд (ответ больше предела на всех узлах — сам
//!   меньше не станет); ИЛИ
//! - отставание больше bsc.receipts_max_lag_blocks держится непрерывно дольше bsc.receipts_stuck_reset_s (лента ползёт,
//!   но не догоняет: 429/троттлинг; таймер сбрасывается только при реальном догоне ≤ max_lag, НЕ на каждом блоке).
//!
//! Пока отставание меньше bsc.receipts_max_lag_blocks, пулы достоверны: запись «данные пулов отстают на N блоков»
//! каждый раз, когда оно выросло ещё на окно опережения, и одна запись «догнала», когда снова не больше окна.
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: ctx.rs.
//! Настройки: через вызывающего — bsc.receipts_ahead_blocks, bsc.receipts_block_max_tries, bsc.receipts_stuck_reset_s.

use std::time::Instant;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::receipts::SOURCE;

/// Блок, на котором стоит конвейер.
struct Stuck {
    n: u64,
    since: Instant,
    tries: u32,
    /// «Жёстких» отказов (ответ больше предела, Fail::Over) подряд на этом блоке — по ним решается пропуск.
    hard: u32,
    why: String,
    /// «Блока нет нигде» подряд (Fail::Missing) — для одного узла невосполнимо только со второго.
    nowhere: u32,
    /// Узлы повтора этого блока уже записаны в журнал (чтобы hot-swap был виден, но не на каждый ретрай).
    hosts_told: bool,
}

/// Отставание ленты и блок, на котором она стоит.
#[derive(Default)]
pub struct Lag {
    stuck: Option<Stuck>,
    /// Отставание, о котором уже записано (0 — не отстаём).
    told: u64,
    /// Стоит отметка «данные пулов отстают» (решение 72).
    marked: bool,
    /// С какого момента отставание больше max_lag непрерывно (решение 77): сбрасывается только при догоне ≤ max_lag,
    /// не на каждом доставленном блоке. По нему — восстановление при «ползёт, но не догоняет».
    behind_since: Option<Instant>,
}

impl Lag {
    /// Блок `n` не пришёл. `why` — причина; `missing` — «блока нет нигде» (иначе счёт обнуляется); `hard` — ответ
    /// больше предела (Fail::Over: такой блок сам меньше не станет). Ошибок нет.
    pub fn failed(&mut self, n: u64, why: &str, missing: bool, hard: bool) {
        match &mut self.stuck {
            Some(s) if s.n == n => {
                s.tries += 1;
                s.why = why.to_string();
                s.nowhere = if missing { s.nowhere + 1 } else { 0 };
                s.hard = if hard { s.hard + 1 } else { 0 };
            }
            _ => {
                self.stuck = Some(Stuck {
                    n,
                    since: Instant::now(),
                    tries: 1,
                    hard: hard as u32,
                    why: why.to_string(),
                    nowhere: missing as u32,
                    hosts_told: false,
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
        }
    }

    /// Нужно ли записать узлы повтора этого блока (один раз на застревание — hot-swap виден, без спама). Ошибок нет.
    pub fn take_hosts_told(&mut self) -> bool {
        match &mut self.stuck {
            Some(s) if !s.hosts_told => {
                s.hosts_told = true;
                true
            }
            _ => false,
        }
    }

    /// Пропуск по блоку (решение 77): блок дал `max` «жёстких» отказов подряд (ответ больше предела на всех узлах).
    /// `max` 0 — не задана настройка: пропуска по блоку нет (сработает триггер по длительности). Причина — или None.
    /// Ошибок нет.
    pub fn overdue_block(&self, max: u32) -> Option<String> {
        let s = self.stuck.as_ref()?;
        (max > 0 && s.hard >= max).then(|| {
            format!(
                "блок {} отвергнут пределом {max} раз подряд (ответ больше предела на всех узлах) — пропуск, пулы пересинхронизируются{}",
                s.n,
                self.stuck_text()
            )
        })
    }

    /// Блок, на котором стоит конвейер (его повтор — гонкой на все узлы). Ошибок нет.
    pub fn hot(&self) -> Option<u64> {
        self.stuck.as_ref().map(|s| s.n)
    }

    /// Для записи при пропуске: на каком блоке и почему лента стояла. Пусто — не стояла. Ошибок нет.
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
    /// (`flag` — events_lag.rs), лента догоняет без сброса; снова не больше — отметка снята. Запись — при смене. Ведёт
    /// таймер непрерывного отставания (решение 77). Ошибок нет.
    pub async fn mark(
        &mut self,
        ctx: &Ctx,
        flag: &super::events_lag::EventsLag,
        behind: u64,
        max_lag: u64,
    ) {
        let over = behind > max_lag;
        flag.set(if over { behind } else { 0 });
        // Таймер непрерывного отставания: ставится при переходе в «отстаём», снимается только при догоне.
        match (over, self.behind_since) {
            (true, None) => self.behind_since = Some(Instant::now()),
            (false, Some(_)) => self.behind_since = None,
            _ => {}
        }
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

    /// Пропуск по длительности (решение 77): отставание больше max_lag держится непрерывно дольше
    /// bsc.receipts_stuck_reset_s — лента ползёт, но не догоняет (429/троттлинг), и «вечно мёртвый блок» тоже сюда.
    /// Настройка не задана — одна запись за застревание, пропуска нет. Принимает: окружение, ленту (событие ядру).
    /// Возвращает: причину пропуска или None. Ошибок нет.
    pub async fn overdue_behind(
        &mut self,
        ctx: &Ctx,
        feed: &super::receipts::Feed,
        no_reset_told: &mut bool,
    ) -> Option<String> {
        let since = self.behind_since?;
        let limit = match ctx.int("bsc.receipts_stuck_reset_s") {
            Ok(v) => v,
            Err(e) => {
                if !std::mem::replace(no_reset_told, true) {
                    let msg = format!(
                        "{SOURCE}: {} — отставание не восстанавливается пропуском",
                        e.message
                    );
                    ctx.log(Level::Warn, "venue.ws", msg, None).await;
                }
                return None;
            }
        };
        if since.elapsed().as_secs() < limit {
            return None;
        }
        let why = format!(
            "отставание больше bsc.receipts_max_lag_blocks держится дольше bsc.receipts_stuck_reset_s = {limit} с — пропуск на голову, пулы пересинхронизируются{}",
            self.stuck_text()
        );
        feed.notice(ctx, "dex.receipts_stuck", format!("{SOURCE}: {why}"))
            .await;
        Some(why)
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
            // Число в момент записи — порог окна, а не фактическое отставание (ревью #65: «отставание 6» при окне 6).
            let msg = format!(
                "{SOURCE}: лента догнала сеть: отставание ≤ окна bsc.receipts_ahead_blocks ({step})"
            );
            ctx.log(Level::Info, "venue.ws", msg, None).await;
        }
    }
}
