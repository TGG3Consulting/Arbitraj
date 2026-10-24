//! Файл: venues/bsc_dex/check/quoter_tally.rs
//! Что делает: счёт итогов первого прохода сверки с квотером и одна сводная запись в журнал после него
//! (вместо записи на каждый пул): прошли, исключены, мельче dex.min_pool_liquidity_usd, самые частые причины.
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: pools/listen.rs (Pools), pools/progress.rs (reason_key).
//! Настройки: notify.summary_reasons (сколько причин в сводке; не задана — сводка без причин, с пометкой).

use std::collections::HashMap;

use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;

/// Итоги прохода.
#[derive(Default)]
pub struct Tally {
    pub ok: usize,
    pub failed: usize,
    /// Мельче dex.min_pool_liquidity_usd — не сверялись.
    pub dust: usize,
    /// Вид причины → сколько пулов.
    reasons: HashMap<String, usize>,
}

impl Tally {
    /// Учитывает итог пула. Ошибок нет.
    pub fn add(&mut self, res: &Result<(), String>) {
        match res {
            Ok(()) => self.ok += 1,
            Err(why) => {
                self.failed += 1;
                *self.reasons.entry(class(why)).or_default() += 1;
            }
        }
    }

    /// Одна сводная запись в журнал. Ошибок нет.
    pub async fn summary(self, p: &Pools) {
        let mut msg = format!(
            "Сверка с квотером (первый проход): прошли {}, исключены {}, мельче dex.min_pool_liquidity_usd (не сверялись) {}",
            self.ok, self.failed, self.dust
        );
        if !self.reasons.is_empty() {
            let top = match p
                .ctx
                .settings
                .integer("notify.summary_reasons", At::default())
            {
                Ok(n) => {
                    let mut v: Vec<(String, usize)> = self.reasons.into_iter().collect();
                    v.sort_by_key(|x| std::cmp::Reverse(x.1));
                    v.iter()
                        .take(n as usize)
                        .map(|(r, n)| format!("{n} — {r}"))
                        .collect::<Vec<_>>()
                        .join("; ")
                }
                Err(e) => format!("не показаны — {}", e.message),
            };
            msg.push_str(&format!(". Причины: {top}"));
        }
        p.ctx.log(Level::Info, "dex.quoter", msg, None).await;
    }
}

/// Вид причины для сводки: расхождение — одним видом, остальное — текст без цифр. Ошибок нет.
fn class(why: &str) -> String {
    if why.contains("dex.quoter_max_diff_pct") {
        "расхождение с квотером больше dex.quoter_max_diff_pct".into()
    } else {
        reason_key(why)
    }
}
