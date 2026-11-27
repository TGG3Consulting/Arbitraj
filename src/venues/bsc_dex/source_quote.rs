//! Файл: venues/bsc_dex/source_quote.rs.
//! Что делает: выбирает источник ручной котировки и переводит его ответ в общие денежные поля.
//! Кто вызывает: единый метод source_quote контракта площадки.
//! От чего зависит: контракт и проверка рынка внутри конкретного источника.
//! Настройки: профиль сети и настройки самого источника.
use super::{ctx::Ctx, hanji, hashflow};
use crate::gateway::{
    errors::{ErrorCode, GatewayResult},
    source_quote::{Fee, Preview, Request},
};

pub(super) async fn preview(ctx: &Ctx, source: &str, req: Request) -> GatewayResult<Preview> {
    let bad = |message| ctx.err(ErrorCode::NotApplicable, message);
    if matches!(source, "kipseli" | "zeroex") {
        return super::keyed_price::preview(ctx, source, req).await;
    }
    if source == "bopamm" {
        return super::fermi::bopamm::preview(ctx, req).await;
    }
    if req.firm.is_some() {
        return Err(bad(
            "У этого источника собственная форма проверки котировки",
        ));
    }
    if source == "native_rfq" {
        return super::native_rfq::preview(ctx, req).await;
    }
    if source == "fynd" {
        return super::fynd::preview(ctx, req).await;
    }
    if source == "fermi" {
        return super::fermi::source_preview(ctx, req).await;
    }
    let Request {
        input,
        output,
        amount_raw,
        amount_tokens,
        trader,
        market,
        builder,
        firm: _,
    } = req;
    match source {
        "hanji" => {
            if builder.is_some() {
                return Err(bad(
                    "Для книги Hanji укажите amount_raw, два токена и при необходимости адрес рынка",
                ));
            }
            let quote = hanji::live::preview(
                ctx,
                hanji::model::Request {
                    input,
                    output,
                    market,
                    amount_raw,
                    amount_tokens,
                    trader,
                },
            )
            .await?;
            let details = serde_json::to_value(&quote).map_err(|_| {
                ctx.err(ErrorCode::Internal, "Ошибка преобразования котировки Hanji")
            })?;
            Ok(Preview {
                source: source.into(),
                input,
                output,
                spent_raw: quote.spent_raw,
                received_raw: quote.received_raw,
                unspent_raw: quote.unspent_raw,
                fee: Some(Fee {
                    asset: quote.fee_asset,
                    amount_raw: quote.fee_raw,
                }),
                block_hash: format!("{:#x}", quote.hash),
                block: Some(quote.block),
                expires: quote.deadline,
                details,
            })
        }
        "hashflow" => {
            if market.is_some() || builder.is_some() {
                return Err(bad("Для Hashflow не применяются market и builder"));
            }
            let trader = trader
                .ok_or_else(|| bad("Для подписанной RFQ требуется публичный адрес трейдера"))?;
            let quote = hashflow::preview(
                ctx,
                hashflow::model::Request {
                    trader,
                    input,
                    output,
                    amount: amount_raw,
                    amount_tokens,
                },
            )
            .await?;
            let details = serde_json::to_value(&quote)
                .map_err(|_| ctx.err(ErrorCode::Internal, "Ошибка преобразования RFQ"))?;
            Ok(Preview {
                source: source.into(),
                input,
                output,
                spent_raw: quote.amount_in,
                received_raw: quote.amount_out,
                unspent_raw: "0".into(),
                fee: None,
                block_hash: quote.block_hash,
                block: None,
                expires: Some(quote.expires),
                details,
            })
        }
        _ => Err(bad(
            "У этой DEX-площадки нет такого источника ручной котировки",
        )),
    }
}
