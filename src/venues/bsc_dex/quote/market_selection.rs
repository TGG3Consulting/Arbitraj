//! Сравнение штатного маршрута с отдельными исполнимыми формами по чистым суммам, газу и разрешениям.
use super::super::{
    ctx::Ctx,
    pools::{family_variant::Variant, pendle_quotes, registry::Entry, safeswap_quotes},
};
use super::{
    Quoting, cpu, gas::GasPrice, quote_timing, quote_timing_outer::Outer, routing, v2_execution,
    v2_fallback, vista_pricing,
};
use crate::{
    core::types::Quote,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy_primitives::{Address, Bytes, U256};

type QuoteRequest = fn(&Ctx, &Entry, Address, U256) -> GatewayResult<(Address, Bytes)>;

/// Обе части ABI выбираются вместе. Этот рынок получает цены от собственного контракта.
pub(super) struct WalletQuoter {
    pub request: QuoteRequest,
    pub payout: fn(&[u8]) -> Option<U256>,
}

pub(super) fn wallet_quoter(variant: Variant) -> Option<WalletQuoter> {
    match variant {
        v if v.euler() => Some(WalletQuoter{request:super::super::pools::euler_quotes::request,payout:super::super::pools::integral_quotes::payout}),
        Variant::Stabull => Some(WalletQuoter{request:super::super::pools::stabull_quotes::request,payout:super::super::pools::integral_quotes::payout}),
        Variant::MaverickOne => Some(WalletQuoter{
            request:super::super::pools::maverick_one_quotes::request,
            payout:super::super::pools::maverick_one_quotes::payout,
        }),
        Variant::PropVenue=>Some(WalletQuoter{request:super::super::pools::prop_quotes::request,payout:super::super::pools::prop_quotes::payout}),
        Variant::IntegralSize=>Some(WalletQuoter{request:super::super::pools::integral_quotes::request,payout:super::super::pools::integral_quotes::payout}),
        Variant::DegenSwap => Some(WalletQuoter{
            request: super::super::pools::degenswap_quotes::request,
            payout: super::super::pools::degenswap_quotes::payout,
        }),
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

/// These order books validate the returned quote against the selected maker allocations.
pub(super) struct BookQuoter {
    pub request: QuoteRequest,
    pub payout: fn(&Entry, Address, U256, &[u8]) -> Option<U256>,
}
pub(super) fn order_book_quoter(variant:Variant)->Option<BookQuoter>{
    use super::super::pools::{carbon_quote,rubicon};
    match variant {
        Variant::Ammalgam=>Some(BookQuoter{request:super::super::pools::ammalgam::request,payout:super::super::pools::ammalgam::payout}),
        _ if variant.fluid_lite()=>Some(BookQuoter{request:super::super::pools::fluid_dex_lite::request,payout:super::super::pools::fluid_dex_lite::payout}),
        _ if variant.ambient()=>Some(BookQuoter{request:super::super::pools::ambient::request,payout:super::super::pools::ambient::payout}),
        Variant::Llamma(_)=>Some(BookQuoter{request:super::super::pools::llamma_quotes::request,payout:super::super::pools::llamma_quotes::payout}),
        Variant::Mooniswap=>Some(BookQuoter{request:super::super::pools::mooniswap::request,payout:super::super::pools::mooniswap::payout}),
        Variant::Brokerbot=>Some(BookQuoter{request:super::super::pools::brokerbot_quotes::request,payout:super::super::pools::brokerbot_quotes::payout}),
        Variant::RubiconClassic=>Some(BookQuoter{request:rubicon::request,payout:rubicon::payout}),
        Variant::Carbon=>Some(BookQuoter{request:carbon_quote::request,payout:carbon_quote::payout}),
        _=>None,
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
        alternatives.extend(Box::pin(super::fluid_hop_pricing::candidates(&pick,&input,native||native_cash)).await);
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
            if native_cash && !vista_pricing::terminal_cash(&self.reg,quote.route.as_ref()) {continue;}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::pools::families;

    #[test]
    fn recorded_fluid_market_uses_the_original_resolver() {
        let raw = include_bytes!("../../../../tests/data/first_run_dex_2026-11-26/fluid_dex_lite_wallet.json.gz");
        let wallet: serde_json::Value = serde_json::from_reader(flate2::read::GzDecoder::new(&raw[..])).unwrap();
        let kind = wallet["manifest"]["kind"].as_str().unwrap();
        let family = families::make(kind, kind).unwrap();
        assert!(family.variant.fluid_lite());
        assert!(order_book_quoter(family.variant).is_some());
        assert!(!super::super::v2_pair_quote::kind_on(family));
    }

    #[test]
    fn wallet_quoters_keep_their_own_abi_including_signal_only_families() {
        for (kind, words) in [
            ("safeswap_trade_module", 2),
            ("pendle_market", 4),
            ("pendle_market_v5", 5),
        ] {
            let f = families::make(kind, kind).unwrap();
            assert!(!super::super::v2_pair_quote::kind_on(f));
            let quoter = wallet_quoter(f.variant).unwrap();
            let mut reply = vec![0; 32 * words];
            reply[31] = 1;
            assert_eq!((quoter.payout)(&reply), Some(U256::ONE));
            assert_eq!((quoter.payout)(&reply[..32]), None);
        }
        for kind in ["maverick_v1","maverick_v1_module"] {
            let f=families::make("maverick",kind).unwrap();
            assert!(!super::super::v2_pair_quote::kind_on(f));
            let quoter=wallet_quoter(f.variant).unwrap();
            let mut reply=[0u8;32];
            assert_eq!((quoter.payout)(&reply),None);
            reply[31]=7;
            assert_eq!((quoter.payout)(&reply),Some(U256::from(7)));
            assert_eq!((quoter.payout)(&reply[..31]),None);
        }
        let fork = families::make("pair", "v2_fork_executor").unwrap();
        assert!(wallet_quoter(fork.variant).is_none());
        assert!(super::super::v2_pair_quote::kind_on(fork));
    }
}
