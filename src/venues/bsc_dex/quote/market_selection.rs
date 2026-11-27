//! Сравнение штатного маршрута с отдельными исполнимыми формами по чистым суммам, газу и разрешениям.
use super::{
    Quoting, cpu, gas::GasPrice, quote_timing, quote_timing_outer::Outer, routing, v2_execution,
    v2_fallback, vista_pricing,
};
use crate::{
    core::types::Quote,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, Bytes, U256};
use super::super::{
    ctx::Ctx,
    pools::{family_variant::Variant, pendle_quotes, registry::Entry, safeswap_quotes},
};

/// Обе части ABI выбираются вместе. Этот рынок получает цены от собственного контракта.
pub(super) struct WalletQuoter {
    pub request: fn(&Ctx, &Entry, Address, U256) -> GatewayResult<(Address, Bytes)>,
    pub payout: fn(&[u8]) -> Option<U256>,
}

pub(super) fn wallet_quoter(variant: Variant) -> Option<WalletQuoter> {
    match variant {
        Variant::SafeSwapTrade => Some(WalletQuoter {
            request: safeswap_quotes::request,
            payout: safeswap_quotes::payout,
        }),
        Variant::Pendle => Some(WalletQuoter {
            request: pendle_quotes::request,
            payout: pendle_quotes::payout,
        }),
        _ => None,
    }
}

impl Quoting {
    pub(super) async fn market_routes(
        &self,
        pick: Self,
        input: routing::Input,
        fees: &GasPrice,
        native: bool,
        tm: &mut Outer,
    ) -> GatewayResult<Quote> {
        let ctx = &self.ctx;
        let (coin_dec, usdt_dec, gas) = (input.coin_dec, input.usdt_dec, input.gas);
        let native_cash = super::super::exec::native::cash_ticker(ctx, &input.usdt_ticker);
        if native_cash && !vista_pricing::cash(ctx, &input.usdt_ticker, input.usdt) {
            return Err(ctx.not_set(
                "dex.native_wrapped",
                "У расчётной нативной монеты другой адрес обёртки",
            ));
        }
        let (mut alternatives, compare_single) = Box::pin(v2_fallback::candidates(
            &pick,
            &input,
            native || native_cash,
        ))
        .await;
        if compare_single && !native_cash && !alternatives.is_empty() {
            v2_fallback::private(&pick, &input, &mut alternatives);
        }
        let work = quote_timing::timed(
            tm.run(),
            v2_fallback::ordinary(pick, input, compare_single, native || native_cash),
        );
        let raw = cpu::run(ctx, Box::pin(work)).await.and_then(|quote|{
            if native_cash && !vista_pricing::terminal_cash(&self.reg,quote.route.as_ref()) {
                return Err(ctx.err(ErrorCode::NoRoute,
                    "Выбранная форма исполнения не подтверждает списание или выплату нативной расчётной валюты"));
            }
            Ok(quote)
        });
        tm.back();
        let mut best = match raw {
            Ok(q) => {
                self.finish(q, (coin_dec, usdt_dec), gas, fees, native, None, tm)
                    .await
            }
            Err(e) => Err(e),
        };
        for (quote, approval) in alternatives {
            let Ok(other) = self
                .finish(quote, (coin_dec, usdt_dec), gas, fees, native, approval, tm)
                .await
            else {
                continue;
            };
            if crate::gateway::quote_stage::settlement()
                && !crate::gateway::quote_stage::settlement_quote_supported(
                    &other,
                    crate::gateway::quote_stage::is_final(),
                )
            {
                continue;
            }
            best = Ok(match best {
                Ok(first) => v2_execution::choose(self, first, other, usdt_dec).await,
                Err(e)
                    if matches!(
                        e.code,
                        ErrorCode::NoRoute
                            | ErrorCode::CoinNotChecked
                            | ErrorCode::NotConfigured
                            | ErrorCode::NotApplicable
                    ) =>
                {
                    other
                }
                Err(e) => return Err(e),
            });
        }
        best
    }
}
