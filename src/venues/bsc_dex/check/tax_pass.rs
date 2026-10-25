//! Файл: venues/bsc_dex/check/tax_pass.rs
//! Что делает: проверка налогов списка монет (один проход или его часть). Итог Known / Failed — в состояние и событие
//! ядру; «ждёт» (нет достоверного пула, сбой узла — 429, нет связи) и «не задана настройка» — прошлый итог не
//! меняется (у монеты без итога — NotChecked с причиной ожидания для экранов), монета — в список ждущих (не вердикт,
//! не чёрный список). Монета, чьи пулы ещё загружаются, ждёт только их (Why::Loading). Журнал по монете — только при смене состояния
//! (прошла ↔ не прошла, сменилась причина без учёта цифр) и не в первом проходе (там — одна сводка, tax_cycle.rs).
//! Ошибка настройки — одна запись в журнал и одно событие dex.check_not_configured на каждую разную причину.
//! Кто вызывает: check/tax_cycle.rs.
//! От чего зависит: check/taxes.rs, check/state.rs, pools/listen.rs (Pools), pools/progress.rs, node/heads.rs.
//! Настройки: через вызываемые.

use std::collections::HashSet;

use alloy_primitives::Address;

use crate::core::types::CheckState;
use crate::gateway::events::MarketEvent;
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::tax_deep::{has_deep, has_valid};
use super::taxes::{self, Outcome};
use super::taxes_util::Sources;

/// Почему монета ждёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Нет достоверного пула — проверка по сигналу «пул готов».
    NoPool,
    /// Пул был, помешал узел или окружение (429, нет связи) — повтор через tax.retry_s.
    Node,
    /// Достоверные пулы есть, но ни одного глубокого в оба направления (решение 73) — повтор не по таймеру, а по
    /// сигналу «пул готов», когда у монеты появился глубокий пул (пересчёт глубины на смене состояния, пробуждение
    /// мелкого пула — pools/admit.rs wake).
    Shallow,
    /// Не задана настройка — до следующего полного прохода.
    Config,
    /// Пулы монеты ещё загружаются — проверка, как только они прочитаны (сигнал «пул готов»).
    Loading,
}

/// Итоги прохода.
#[derive(Default)]
pub struct Pass {
    /// Ждущие монеты: адрес, тикер, почему.
    pub waiting: Vec<(Address, String, Why)>,
    pub ok: usize,
    pub failed: usize,
    pub no_pool: usize,
    pub config: usize,
    /// Монеты, чьи пулы ещё загружаются (проверки не было).
    pub loading: usize,
    /// Монеты, по которым проверка была (итог или причина ожидания — не «пулы загружаются»): адрес и вид итога —
    /// для сводки первого прохода (tax_cycle.rs).
    pub done: Vec<(Address, Res)>,
}

/// Вид итога монеты для сводки первого прохода.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Res {
    Ok,
    Failed,
    Wait,
    Config,
}

/// Проверяет монеты списка (см. шапку). Монета, чьи пулы ещё загружаются (pools/loading.rs), не проверяется — ждёт
/// своих пулов, а не пулов всех монет (решение 61). Принимает: окружение, монеты, USDT, уже записанные ошибки
/// настроек, монеты, у которых ещё не было первой проверки (журнал по ним — только сводкой). Ошибок нет.
pub async fn check(
    p: &Pools,
    list: &[(Address, String)],
    usdt: Address,
    seen: &mut HashSet<String>,
    firsts: &HashSet<Address>,
) -> Pass {
    let mut pass = Pass::default();
    // Балансы источников пробы — один раз на проход (check/taxes_util.rs).
    let sources = Sources::default();
    for (i, (addr, ticker)) in list.iter().enumerate() {
        let first = firsts.contains(addr);
        let left = p.loading.pending(p, *addr);
        if left > 0 {
            pass.loading += 1;
            let why = format!("пулы монеты ещё загружаются ({left})");
            super::taxes_util::note_wait(
                p,
                &p.checks.coin(ticker).map(|c| c.taxes),
                super::taxes_util::loading_check(p, ticker, *addr, why),
            )
            .await;
            pass.waiting.push((*addr, ticker.clone(), Why::Loading));
            continue;
        }
        // Данные пулов отстают (лента догоняет, решение 72) — вердикта нет: известный итог не меняется, проверка — по
        // сигналу «пул готов» (его даёт и снятие отметки).
        if p.lag.busy() {
            let why = match p.lag.get() {
                0 => "лента квитанций сброшена, пулы перечитываются — проверка после перечитывания"
                    .to_string(),
                n => format!("данные пулов отстают на {n} блоков — проверка после догона"),
            };
            super::taxes_util::note_wait(
                p,
                &p.checks.coin(ticker).map(|c| c.taxes),
                super::taxes_util::loading_check(p, ticker, *addr, why),
            )
            .await;
            pass.no_pool += 1;
            pass.waiting.push((*addr, ticker.clone(), Why::NoPool));
            continue;
        }
        // Узел отвечает и время блока есть — иначе проход откладывается (срок пробы берётся свежим в probe.rs).
        match p.heads.at(&p.ctx).await {
            Ok(_) => {}
            Err(e) => {
                let msg = format!("Проверка налогов отложена: {}", e.message);
                p.signals.once(&p.ctx, Level::Warn, "dex.check", msg).await;
                for (a, t) in &list[i..] {
                    pass.waiting.push((*a, t.clone(), Why::Node));
                }
                pass.no_pool += list.len() - i;
                return pass;
            }
        }
        let (c, out) = taxes::check_coin(
            &p.ctx, &p.reg, &p.checks, &p.tokens, &sources, ticker, *addr, usdt, &p.heads,
        )
        .await;
        let prev = p.checks.coin(ticker).map(|c| c.taxes);
        match out {
            Outcome::Known(_) => {
                pass.ok += 1;
                pass.done.push((*addr, Res::Ok));
                if !first && matches!(prev, Some(CheckState::Failed { .. })) {
                    let msg = format!("{ticker}: проверка налогов снова прошла");
                    p.ctx.log(Level::Info, "dex.check", msg, None).await;
                }
            }
            Outcome::Failed(reason) => {
                pass.failed += 1;
                pass.done.push((*addr, Res::Failed));
                let same = matches!(&prev, Some(CheckState::Failed { reason: r }) if reason_key(r) == reason_key(&reason));
                if !first && !same {
                    let msg = format!("{ticker}: проверка налогов не прошла: {reason}");
                    p.ctx.log(Level::Warn, "dex.check", msg, None).await;
                }
            }
            Outcome::Wait(_) => {
                pass.no_pool += 1;
                pass.done.push((*addr, Res::Wait));
                // Причину ожидания видно на «Монетах» и «Живом экране»: монета без итога получает NotChecked с причиной;
                // уже известный итог (прошла / не прошла) из-за помехи не меняется.
                super::taxes_util::note_wait(p, &prev, c).await;
                // Нет глубокого пула — своя причина (не сбой узла): повтор по «пул готов», не каждые tax.retry_s.
                let why = if !has_valid(p, ticker) {
                    Why::NoPool
                } else if !has_deep(p, ticker) {
                    Why::Shallow
                } else {
                    Why::Node
                };
                pass.waiting.push((*addr, ticker.clone(), why));
                continue;
            }
            Outcome::Config(why) => {
                pass.config += 1;
                pass.done.push((*addr, Res::Config));
                super::taxes_util::note_wait(p, &prev, c.clone()).await;
                pass.waiting.push((*addr, ticker.clone(), Why::Config));
                if seen.insert(why.clone()) {
                    let text = format!("Проверка налогов: не задана настройка — {why}");
                    p.ctx
                        .log(Level::Error, "dex.check_not_configured", text.clone(), None)
                        .await;
                    let code = "dex.check_not_configured".to_string();
                    let venue = p.ctx.venue();
                    let _ = p.out.try_send(MarketEvent::Notice { venue, code, text });
                }
                continue;
            }
        }
        p.checks.set_coin(c.clone());
        // С ожиданием места в канале: итог проверки не должен теряться, когда ядро занято (первый проход — сотни монет).
        if p.out.send(MarketEvent::CoinCheck(c)).await.is_err() {
            return pass;
        }
    }
    pass
}
