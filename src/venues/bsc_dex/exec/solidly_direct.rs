//! Файл: venues/bsc_dex/exec/solidly_direct.rs.
//! Что делает: строит один вызов проверенного модуля и сохраняет резерв перед первой публикацией подписанного обмена.
//! Кто вызывает: plan, swap_send, txs и bundle_send. От чего зависит: nonce, queue_store и ABI проверенного модуля.
//! Настройки: dex.router, dex.deadline_s, dex.approve_unlimited и кошелёк площадки.
use super::super::ctx::Ctx;
use super::super::pools::direct_module;
use super::super::wallet::{approve, nonce::Slot, queue_store};
use super::{chain::Step, plan::Plan};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy_primitives::{Address, B256, U256};
use std::future::Future;

tokio::task_local! { pub(super) static SENDING: (); }

pub(super) async fn plan(
    ctx: &Ctx,
    steps: &[Step],
    wallet: Address,
    now: u64,
    all: bool,
) -> GatewayResult<Plan> {
    let [step] = steps else {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Прямой модуль исполняет один рынок за вызов",
        ));
    };
    if !step.entry.family.tradable || !step.from_wallet || !step.to_wallet {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Прямому модулю требуется полный шаг ERC-20 кошелька",
        ));
    }
    let pool = direct_module::market(ctx, &step.entry)?;
    let router = ctx.dex_addr("dex.router", step.entry.family.id)?;
    if step.entry.family.variant==super::super::pools::family_variant::Variant::EmpireModule {
        let at=super::super::pools::nami::block(ctx,"latest").await?;
        tokio::try_join!(super::super::pools::empire::fresh(ctx,&step.entry,&at),direct_module::verify_factory(ctx,step.entry.family,router,&at))?;
    }else{direct_module::verify(ctx, step.entry.family, router, "latest").await?;}
    let order = &step.order;
    let approvals = if all {
        approve::fork_all(ctx, order.token_in, router, order.amount_in)?
    } else {
        approve::fork_missing(ctx, order.token_in, wallet, router, order.amount_in).await?
    };
    let deadline = U256::from(now + ctx.int("dex.deadline_s")?);
    let mut plan = Plan::new(
        ctx,
        (approvals, router, wallet),
        order.token_out,
        Vec::new(),
        order.min_out,
        deadline,
    )?;
    plan.set_direct_market(super::plan::DirectCall::Module {
        pool,
        input: order.token_in,
        amount: order.amount_in,
    });
    Ok(plan)
}

/// Сначала записывает nonce и хеш. Отмена future во время публикации сохраняет остановку через перезапуск.
pub async fn dispatch<T>(
    ctx: &Ctx,
    slot: &Slot,
    hash: B256,
    nonce: u64,
    outgoing: impl Future<Output = GatewayResult<T>>,
) -> GatewayResult<T> {
    let hold = slot.hold();
    let state = hold.halt(
        "Подписан обмен через прямой модуль, ожидается исход передачи".into(),
        nonce,
        hash,
    );
    if let Err(why) = queue_store::save_checked(ctx, &state).await {
        hold.free();
        return Err(ctx.err(
            ErrorCode::Internal,
            format!("Запись резерва перед отправкой: {why}"),
        ));
    }
    let result = outgoing.await;
    if let Err(e) = &result
        && matches!(
            e.code,
            ErrorCode::ChannelRejected | ErrorCode::NotConfigured
        )
    {
        queue_store::save(ctx, &hold.free()).await;
    }
    result
}
