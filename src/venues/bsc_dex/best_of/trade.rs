//! Файл: venues/bsc_dex/best_of/trade.rs
//! Что делает: сделка DEX в режиме «лучший из двух» (решение 123). Сравнения на котировке мало — к сделке цены
//! сдвинулись, поэтому в один момент параллельно: подготовка свопа KyberSwap (kyberswap/trade_prep.rs — тот самый
//! /routes + /route/build, что пойдёт в его своп, второго запроса нет) и свой свежий финальный маршрут (Quoting::quote).
//! Сравнение тем же choose.rs (с худшей ценой ядра req.worst_price) и исполнение победителя со всеми его проверками:
//! KyberSwap — kyberswap/trade.rs execute (approve и пересборка, решение 99); свой — exec/trade.rs по свежему маршруту
//! (верификация, минимумы по худшей цене exec/leg_min.rs, пересборка после approve, симуляция, проверка исполнителя
//! exec/executor/onchain.rs). Свой отказал ДО отправки свопа (pre_send) — в той же сделке KyberSwap по уже
//! подготовленному маршруту (он прошёл проверку худшей цены). После отправки — повторов нет (стартовое правило
//! Тиграна). Кто исполнил и почему — TradeResult.executor и начало текста ошибки (хронология сделки), журнал dex.best_of.
//! Кто вызывает: venue.rs (trade при exec.via_kyber = да и exec.dex_best_of = да).
//! От чего зависит: kyberswap/{trade, trade_prep}.rs, exec/trade.rs, quote/mod.rs, best_of/{choose, extra}.rs.
//! Настройки: через вызываемые — kyber.*, exec.*, dex.*, send.*.

use crate::core::types::{TradeRequest, TradeResult};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::urgency::{Urgency as Class, scope};
use crate::storage::journal::Level;
use crate::venues::kyberswap::{self, trade_prep::Prepared};

use super::super::exec;
use super::super::parts::Parts;
use super::choose::{self, Offer, Who};
use super::extra;

/// Сделка «лучший из двух». Принимает: состояние bsc_dex, состояние KyberSwap, запрос ядра. Возвращает: факт свопа
/// победителя. Ошибки: обе стороны не годятся — ошибка KyberSwap (иначе своя) с причинами обеих; ошибки исполнителя
/// (с фактом транзакции) — подписаны им и причиной выбора.
pub async fn trade(
    p: &Parts,
    kyber: &kyberswap::state::Parts,
    req: TradeRequest,
) -> GatewayResult<TradeResult> {
    let q = p.quoting();
    let (k, o) = tokio::join!(
        kyberswap::trade_prep::prepare(kyber, &req),
        scope(Class::Trade, q.quote(&req.asset, req.side, req.qty))
    );
    let (ke, oe) = tokio::join!(
        async {
            match &k {
                Ok(x) => extra::usdt(&q, &x.view).await,
                Err(_) => None,
            }
        },
        async {
            match &o {
                Ok(x) => extra::usdt(&q, x).await,
                Err(_) => None,
            }
        }
    );
    let ko = match &k {
        Ok(x) => Ok(Offer {
            quote: &x.view,
            extra_usdt: ke,
        }),
        Err(e) => Err(e.message.clone()),
    };
    let oo = match &o {
        Ok(x) => Ok(Offer {
            quote: x,
            extra_usdt: oe,
        }),
        Err(e) => Err(e.message.clone()),
    };
    let c = match choose::choose(req.side, Some(req.worst_price), ko, oo) {
        Ok(c) => c,
        Err(why) => {
            let mut e = match (k, o) {
                (Err(e), _) | (_, Err(e)) => e,
                // Обе котировки есть, но обе хуже худшей цены / своя неисполнима — ошибка своей с причиной.
                (Ok(_), Ok(_)) => p.ctx.err(ErrorCode::QuoteMismatch, ""),
            };
            e.message = format!("[лучший из двух] своп не отправлен: {why}");
            return Err(e);
        }
    };
    log(p, &req, &format!("выбор на сделке: {}", c.why)).await;
    match (c.who, k, o) {
        (Who::Kyber, Ok(prep), _) => run_kyber(kyber, req, prep, &c.why).await,
        (Who::Own, k, Ok(own)) => {
            let mut r = req.clone();
            r.route = own.route;
            let env = p.env();
            let res = scope(Class::Trade, exec::trade::trade(&q, &env, &p.nonces, r)).await;
            match (res, k) {
                // Свой отказал ДО отправки свопа — KyberSwap в той же сделке (его маршрут уже в пределах худшей цены).
                (Err(e), Ok(prep)) if pre_send(e.code, e.tx.is_some()) => {
                    let why = format!("{}; свой не отправлен ({}) → KyberSwap", c.why, e.message);
                    log(p, &req, &why).await;
                    run_kyber(kyber, req, prep, &why).await
                }
                (res, _) => tag(res, &c.why),
            }
        }
        // Недостижимо: choose выбирает только годную сторону.
        _ => Err(p.ctx.err(
            ErrorCode::Internal,
            "лучший из двух: победитель без маршрута",
        )),
    }
}

/// Отказ своего исполнителя ДО отправки свопа (симуляция, проверка цены, верификация, проверка исполнителя): своп в
/// сеть не уходил — в той же сделке можно исполнить KyberSwap. Транзакция в ошибке есть (своп ушёл) или код не из
/// «отказов до отправки» — нельзя (стартовое правило Тиграна: отправлено и не исполнилось — больше не пробуем).
/// Принимает: код ошибки, есть ли в ней транзакция. Возвращает: да/нет. Ошибок нет.
pub fn pre_send(code: ErrorCode, has_tx: bool) -> bool {
    !has_tx
        && matches!(
            code,
            ErrorCode::NoRoute
                | ErrorCode::QuoteMismatch
                | ErrorCode::StaleData
                | ErrorCode::CoinNotChecked
                | ErrorCode::BadResponse
                | ErrorCode::InvalidAddress
                | ErrorCode::Internal
        )
}

/// Своп KyberSwap по подготовленному маршруту под строгой очередью кошелька (общий замок с bsc_dex, решение 112).
/// Ошибки: как у kyberswap::trade::execute — подписаны «[KyberSwap]» и причиной выбора.
async fn run_kyber(
    kyber: &kyberswap::state::Parts,
    req: TradeRequest,
    prep: Prepared,
    why: &str,
) -> GatewayResult<TradeResult> {
    let res = {
        let _guard = kyber.wallet_lock.lock().await;
        kyberswap::trade::execute(kyber, req, prep).await
    };
    match res {
        Err(mut e) => {
            e.message = format!("[{}] {}", kyberswap::trade_fact::BY_KYBER, e.message);
            tag(Err(e), why)
        }
        ok => tag(ok, why),
    }
}

/// Причина выбора — к подписи исполнителя (успех) или к тексту ошибки. Ошибок своих нет.
fn tag(r: GatewayResult<TradeResult>, why: &str) -> GatewayResult<TradeResult> {
    match r {
        Ok(mut t) => {
            let who = t.executor.take().unwrap_or_default();
            t.executor = Some(format!("{who}; лучший из двух: {why}"));
            Ok(t)
        }
        Err(mut e) => {
            e.message = format!("{} (лучший из двух: {why})", e.message);
            Err(e)
        }
    }
}

/// Запись выбора в журнал (вид dex.best_of). Ошибок нет.
async fn log(p: &Parts, req: &TradeRequest, text: &str) {
    let msg = format!(
        "{} {:?} {} ({}): {text}",
        req.deal_id, req.side, req.qty, req.asset.ticker
    );
    p.ctx.log(Level::Info, "dex.best_of", msg, None).await;
}
