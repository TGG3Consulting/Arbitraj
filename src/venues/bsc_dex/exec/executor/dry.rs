//! Файл: venues/bsc_dex/exec/executor/dry.rs
//! Что делает: сборка «сухой сделки DEX» (решение 121, C4) через исполнитель ArbExecutor (решение 122, часть X):
//! - «как сделка»: графовый маршрут формы «бандл» при заданном exec.executor_address — вызов развёрнутого исполнителя
//!   (разрешения — недостающие, с узла), как пойдёт сделка (exec/trade.rs);
//! - «через исполнитель» (режим проверки ДО развёртывания): ЛЮБОЙ маршрут котировки собирается шагами исполнителя, а в
//!   симуляцию eth_simulateV1 подставляются код исполнителя (contracts/out/ArbExecutor.bin-runtime, хеш сверен) и слот
//!   владельца = кошелёк бота (stateOverrides) на свободном адресе — том, который исполнитель получит при развёртывании
//!   с кошелька бота сейчас (CREATE: адрес из кошелька и его nonce, кода там нет — проверяется). Разрешение ERC-20 этому
//!   адресу — первым вызовом той же симуляции. Так исполнитель проверяется на живых пулах, не будучи развёрнутым.
//!
//! Кто вызывает: exec/dry_build.rs.
//! От чего зависит: exec/executor/{call, code, deploy}.rs, check/simulate.rs (Override), wallet/approve.rs, node/rpc.rs.
//! Настройки: exec.executor_address [bsc_dex]; через вызываемые — dex.deadline_s, dex.approve_unlimited.

use alloy_primitives::{Address, B256, Bytes};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::check::simulate::Override;
use super::super::super::ctx::Ctx;
use super::super::super::node::rpc;
use super::super::super::wallet::approve::{self, Approval};
use super::super::chain::Step;
use super::call::{self, ExecCall};
use super::{code, deploy};

/// Режим сухой сделки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DryMode {
    /// Как пойдёт сделка (бандл / исполнитель по exec.executor_address / Universal Router).
    AsTrade,
    /// Любой маршрут — через исполнитель с подставленным кодом (до развёртывания).
    Executor,
}

/// Собранный вызов исполнителя для симуляции.
pub struct ExecTx {
    pub call: ExecCall,
    /// Разрешения ERC-20 исполнителю (первыми вызовами симуляции).
    pub approvals: Vec<Approval>,
    /// Подмена: код и владелец на свободном адресе (режим Executor); None — развёрнутый исполнитель.
    pub over: Option<Override>,
    /// Форма маршрута для показа.
    pub form: String,
}

/// Собирает вызов исполнителя. Принимает: доли (шаги exec/chain.rs с минимумами), кошелёк, время блока, режим, адрес из
/// exec.executor_address (режим AsTrade). Ошибки: NoRoute (пул без шага исполнителя, адрес занят кодом), QuoteMismatch,
/// NotConfigured, Internal (артефакт не сверен по хешу), узел.
pub async fn build(
    ctx: &Ctx,
    legs: &[Vec<Step>],
    wallet: Address,
    now: u64,
    (mode, deployed): (DryMode, Option<Address>),
) -> GatewayResult<ExecTx> {
    let internal = |w: String| ctx.err(ErrorCode::Internal, w);
    match (mode, deployed) {
        (DryMode::AsTrade, Some(x)) => {
            let call = call::build(ctx, legs, x, now)?;
            let approvals =
                approve::executor_missing(ctx, call.token_in, wallet, x, call.amount_in).await?;
            let form = format!("граф: исполнитель {x:#x}");
            Ok(ExecTx {
                call,
                approvals,
                over: None,
                form,
            })
        }
        (DryMode::AsTrade, None) => Err(internal(
            "исполнитель не задан (exec.executor_address)".into(),
        )),
        (DryMode::Executor, _) => {
            let nonce = deploy::next_nonce(ctx, wallet).await?;
            let at = wallet.create(nonce);
            let have: Bytes =
                rpc::call(ctx, "eth_getCode", serde_json::json!([at, "latest"])).await?;
            if !have.is_empty() {
                let why = format!(
                    "адрес {at:#x} (кошелёк, nonce {nonce}) уже с кодом — подставить исполнитель некуда"
                );
                return Err(ctx.err(ErrorCode::NoRoute, why));
            }
            let over = Override {
                address: at,
                code: code::runtime().map_err(internal)?,
                state: vec![(
                    code::owner_slot().map_err(internal)?,
                    B256::left_padding_from(wallet.as_slice()),
                )],
            };
            let call = call::build(ctx, legs, at, now)?;
            let approvals = approve::executor_all(ctx, call.token_in, at, call.amount_in)?;
            let form = format!(
                "через исполнитель: код подставлен в симуляцию по адресу {at:#x} (nonce {nonce})"
            );
            Ok(ExecTx {
                call,
                approvals,
                over: Some(over),
                form,
            })
        }
    }
}
