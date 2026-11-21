//! Файл: venues/bsc_dex/best_of/quote.rs
//! Что делает: финальная котировка расчёта кандидата в режиме «лучший из двух» (решение 123): параллельно котировка
//! KyberSwap (kyberswap/quote.rs, как при exec.via_kyber) и своя (Quoting::quote — та же, что при exec.via_kyber = нет),
//! их затраты вне цены в USDT (extra.rs), выбор (choose.rs) и победитель в формате ядра от имени bsc_dex с текстом
//! выбора в Quote.pick. Ядро о площадках не знает: получает обычную котировку. Ошибка KyberSwap (429, срок, нет
//! маршрута) при годной своей — своя; обе не годятся — ошибка KyberSwap (как без режима) с причинами обеих.
//! Кто вызывает: venue.rs (quote при gateway::quote_stage::is_final, exec.via_kyber = да и exec.dex_best_of = да).
//! От чего зависит: kyberswap/quote.rs, quote/mod.rs (Quoting), best_of/{choose, extra}.rs, gateway/urgency.rs.
//! Настройки: через вызываемые — kyber.*, dex.*, venue.quote_asset.

use rust_decimal::Decimal;

use crate::core::types::{Asset, Quote, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::gateway::urgency::{self, Urgency as Class, scope};
use crate::venues::kyberswap;

use super::super::parts::Parts;
use super::choose::{self, Offer, Who};
use super::extra;

/// Финальная котировка «лучший из двух». Принимает: состояние bsc_dex, состояние KyberSwap, монету, сторону,
/// количество. Возвращает: котировку победителя (venue = bsc_dex, pick — почему). Ошибки: обе стороны не годятся —
/// ошибка KyberSwap (если была, иначе своя) с текстом «лучший из двух: KyberSwap: …; свой: …».
pub async fn quote(
    p: &Parts,
    kyber: &kyberswap::state::Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
) -> GatewayResult<Quote> {
    let q = p.quoting();
    // Своя — классом ядра (Trade на пути сделки) или Quote, как без режима; KyberSwap — свой темп (kyber.rps).
    let class = urgency::is_set().unwrap_or(Class::Quote);
    let (k, o) = tokio::join!(
        kyberswap::quote::quote(kyber, asset, side, qty),
        scope(class, q.quote(asset, side, qty))
    );
    let (ke, oe) = tokio::join!(
        async {
            match &k {
                Ok(x) => extra::usdt(&q, x).await,
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
    let choice = choose::choose(side, None, offer(&k, ke), offer(&o, oe));
    match choice {
        Ok(c) => {
            let mut w = match (c.who, k, o) {
                (Who::Kyber, Ok(x), _) | (Who::Own, _, Ok(x)) => x,
                // Недостижимо: choose выбирает только годную сторону.
                _ => {
                    return Err(p.ctx.err(
                        ErrorCode::Internal,
                        "лучший из двух: победитель без котировки",
                    ));
                }
            };
            // Котировка от имени площадки bsc_dex: ядро сверяет площадку стороны.
            w.venue = p.ctx.venue();
            w.pick = Some(c.why);
            Ok(w)
        }
        Err(why) => {
            let mut e = match (k, o) {
                (Err(e), _) | (_, Err(e)) => e,
                _ => p.ctx.err(ErrorCode::NoRoute, String::new()),
            };
            e.message = format!("лучший из двух: {why}");
            Err(e)
        }
    }
}

/// Предложение для choose.rs из котировки или ошибки (текст). Ошибок нет.
fn offer(r: &GatewayResult<Quote>, extra_usdt: Option<Decimal>) -> Result<Offer<'_>, String> {
    match r {
        Ok(quote) => Ok(Offer { quote, extra_usdt }),
        Err(e) => Err(e.message.clone()),
    }
}
