//! Textile FX. Публичная цена, индивидуальный запрос владельца, проверка расчёта и выбор сохранённой заявки.
mod client;
mod profile;
pub(super) mod receipt;
pub(super) mod routing;
#[cfg(test)]
mod tests;
mod trade;
mod wire;
use super::{
    ctx::Ctx, exec::signed_call::Erc20Call, num, parts::Parts, pools::exact_wallet_tokens,
    quote::white_state, wallet,
};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    source_quote::{Fee, Preview, Request},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Bytes, U256, keccak256};
pub(super) use routing::Book;
use serde_json::json;
pub(super) use trade::trade;
pub(super) const LABEL: &str = "Textile FX";
pub(super) fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("textile.trading").unwrap_or(false)
}
pub(super) fn current(expiry: u64) -> bool {
    chrono::Utc::now().timestamp().unsigned_abs() < expiry
}

pub(super) async fn request(p: &Parts, r: Request) -> GatewayResult<Preview> {
    let ctx = &p.ctx;
    let generation = ctx.settings.generation();
    if r.input == r.output
        || r.input.is_zero()
        || r.output.is_zero()
        || r.market.is_some()
        || r.builder.is_some()
    {
        return Err(client::fail(ctx, "Для Textile укажите два разных ERC20"));
    }
    let exact_out = r.buy_amount_raw.is_some();
    let amount = if exact_out {
        if r.amount_raw.is_some() || r.amount_tokens.is_some() {
            None
        } else {
            num::exact_amount(r.buy_amount_raw.as_deref(), None, 0)
        }
    } else if r.amount_tokens.is_some() {
        num::exact_amount(
            r.amount_raw.as_deref(),
            r.amount_tokens.as_deref(),
            p.tokens.decimals(ctx, r.input).await?,
        )
    } else {
        num::exact_amount(r.amount_raw.as_deref(), None, 0)
    }
    .ok_or_else(|| client::fail(ctx, "Укажите один положительный размер Textile"))?;
    let q = client::Query {
        input: r.input,
        output: r.output,
        amount,
        exact_out,
    };
    let mut report = Preview {
        source: "textile".into(),
        input: r.input,
        output: r.output,
        spent_raw: String::new(),
        received_raw: String::new(),
        unspent_raw: "0".into(),
        fee: None,
        block_hash: String::new(),
        block: None,
        expires: None,
        details: json!({"type":"indicative","reserved":false}),
    };
    let (paid, got, fee) = if let Some(t) = r.firm {
        let bound = p.bound(&t.wallet);
        let c = &bound.ctx;
        c.bound()?;
        let owner = wallet::signer::address(c)?;
        let minimum = num::exact_amount(Some(&t.minimum_raw), None, 0)
            .ok_or_else(|| client::fail(c, "Не задан минимум расчёта Textile"))?;
        let cap = if exact_out {
            num::exact_amount(t.maximum_raw.as_deref(), None, 0)
        } else if t.maximum_raw.is_none() {
            Some(amount)
        } else {
            None
        };
        let cap =
            cap.ok_or_else(|| client::fail(c, "Для точного выхода нужен предел расхода Textile"))?;
        if r.trader != Some(owner) || t.fee_tenths_bps.is_some() || (t.allow_trade && !enabled(c)) {
            return Err(client::fail(
                c,
                "Проверьте кошелёк и разрешение торговли Textile",
            ));
        }
        let head = white_state::head(c, "latest").await?;
        let profile = profile::at(c, head, owner).await.ok_or_else(|| {
            client::fail(c, "Не подтверждены контракты Textile на выбранном блоке")
        })?;
        if t.allow_trade && !profile.protected {
            exact_wallet_tokens::check(c, "textile", q.input, q.output, &head.selector()).await?;
        }
        let destination = if profile.protected {
            profile.taker
        } else {
            profile.reactor
        };
        // Фирменный запрос резервирует объём maker. До него проверяем наличие входа и разрешения у своего кошелька.
        use super::node::{abi::IERC20, call};
        let at = head.selector();
        let (balance, allowance) = tokio::try_join!(
            call::canonical(c, &at, q.input, IERC20::balanceOfCall { owner }),
            call::canonical(
                c,
                &at,
                q.input,
                IERC20::allowanceCall {
                    owner,
                    spender: destination
                }
            )
        )?;
        let estimate = client::preview(c, &q).await?;
        if estimate.0 > cap
            || estimate.0 > balance
            || estimate.0 > allowance
            || estimate.1 < minimum
        {
            return Err(c.err(
                ErrorCode::NoRoute,
                "Textile: денег, разрешения или выхода preview недостаточно для заявки",
            ));
        }
        let f = client::firm(c, &q, &profile).await?;
        if f.debit > cap || f.credit < minimum {
            return Err(c.err(
                ErrorCode::NoRoute,
                "Индивидуальные условия Textile выходят за пределы владельца",
            ));
        }
        let floor = minimum.max(f.credit);
        let data: Bytes = if profile.protected {
            wire::IWallet::settleCall {
                terms: wire::Terms {
                    sell: q.input,
                    buy: q.output,
                    takerPays: f.debit,
                    minimum: floor,
                    deadline: U256::from(f.expiry),
                },
                action: f.action.clone(),
            }
            .abi_encode()
            .into()
        } else {
            f.action.clone()
        };
        let proof = receipt::Saved {
            call: Erc20Call {
                chain: profile.chain,
                payer: owner,
                module: destination,
                calldata: keccak256(&data),
            },
            reactor: profile.reactor,
            collector: profile.collector,
            action: keccak256(&f.action),
            input: q.input,
            output: q.output,
            debit: f.debit,
            minimum: floor,
            expiry: f.expiry,
            slices: f.slices,
        };
        let latest = white_state::head(c, "latest").await?;
        let (received, gas) = proof.measure(c, latest, &data).await?;
        if generation != c.settings.generation() || !current(f.expiry) {
            return Err(c.err(
                ErrorCode::NoRoute,
                "Срок или настройки заявки Textile изменились",
            ));
        }
        let id = proof.call.calldata;
        if t.allow_trade {
            let di = bound.tokens.decimals(c, q.input).await?;
            let dout = bound.tokens.decimals(c, q.output).await?;
            p.textile.put(routing::Ticket {
                proof,
                data,
                received,
                gas,
                generation,
                di,
                dout,
            });
        }
        report.block = Some(latest.number.to());
        report.block_hash = format!("{:#x}", latest.hash);
        report.expires = Some(f.expiry);
        report.details = json!({"type":"wallet_simulation","reserved":true,"owner_admitted":t.allow_trade,"ticket":if t.allow_trade{Some(id)}else{None},"gas_used":gas,"taker":profile.taker,"protected":profile.protected});
        (f.debit, received, f.fee)
    } else {
        client::preview(ctx, &q).await?
    };
    // Цена за единицу из полного списания с комиссией и выдачи. Для неизвестной точности остаются исходные raw суммы.
    let (di, dout) = tokio::join!(p.tokens.decimals(ctx, q.input), p.tokens.decimals(ctx, q.output));
    if let (Ok(di), Ok(dout)) = (di, dout)
        && let Some(rate) = wire::output_per_input(paid, di, got, dout)
    {
        report.details["output_per_input"] = json!(rate.normalize().to_string());
    }
    report.spent_raw = paid.to_string();
    report.received_raw = got.to_string();
    report.fee = Some(Fee {
        asset: q.input,
        amount_raw: fee.to_string(),
    });
    Ok(report)
}
