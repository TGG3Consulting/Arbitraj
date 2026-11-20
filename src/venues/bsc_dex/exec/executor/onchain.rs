//! Файл: venues/bsc_dex/exec/executor/onchain.rs
//! Что делает: проверка развёрнутого исполнителя по адресу exec.executor_address (ревью 122, пункт 1) — адрес вводится
//! руками, поэтому до любого approve и до сделки сверяется: код по адресу (eth_getCode) побайтно равен
//! contracts/out/ArbExecutor.bin-runtime (как после развёртывания, deploy.rs) — версия v3 (решение 395, t181), или
//! contracts/out/v2/ArbExecutor.bin-runtime — версия v2, или contracts/out/v1/ArbExecutor.bin-runtime — версия v1
//! (решения 272 и 395: развёрнутые раньше исполнители работают как раньше), и слот владельца (раскладка solc, code.rs::owner_slot; у v1 и v2 — слот 0) хранит кошелёк бота. Не
//! совпало — ни разрешения, ни сделки, ошибка с точной причиной. Сверенная версия записывается у кошелька
//! (settings/accounts/edit_version.rs) — по ней котировка решает, идёт ли нативный BNB через исполнитель (mod.rs).
//! Ревью 272 (А1): у v2 и v3 читается и слот wnative (обёртка из конструктора) — пишется у кошелька рядом с версией и
//! сверяется с dex.native_wrapped (mod.rs::native_block); несовпадение — нативный BNB через исполнитель не идёт, причина
//! в тексте проверки; ERC-20 путь не затронут.
//! Успешная проверка помнится по паре (адрес, кошелёк) — до смены настройки или кошелька (другая пара — проверка
//! заново); отказ не запоминается. Кнопка «Проверить подключение» исполнителя (экран площадки DEX) — probe.
//! Кто вызывает: exec/executor/{trade, dry}.rs (ensure), venue.rs (check_executor → probe).
//! От чего зависит: exec/executor/{code, deploy_gate, live_proof, mod}.rs, node/rpc.rs, wallet/signer.rs, settings/accounts (версия у кошелька).
//! Настройки: exec.executor_address [bsc_dex].
//!
//! eth_getCode / eth_getStorageAt — execution-apis 16caee27 src/eth/state.yaml.

use std::sync::Mutex;

use alloy_primitives::{Address, B256, Bytes};
use serde_json::json;

use crate::gateway::contract::ConnectionCheck;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::super::ctx::Ctx;
use super::super::super::node::rpc;
use super::super::super::wallet::signer;
use super::code::{self, Version};
use super::{deploy_gate, live_proof};

/// Последняя успешно проверенная пара (адрес исполнителя, кошелёк) площадки DEX (решение 435, 433-и: исполнитель с тем
/// же адресом в другой сети — другой контракт), версия кода по адресу и (ревью 272 А1) обёртка из хранилища v2 (None у
/// v1).
type Checked = (&'static str, Address, Address, Version, Option<Address>);
static CHECKED: Mutex<Option<Checked>> = Mutex::new(None);

/// Исполнитель по адресу — наш и принадлежит кошельку. Принимает: адрес исполнителя, кошелёк бота.
/// Возвращает: версию кода (решение 272). Ошибки: InvalidAddress (нет кода / код не тот / владелец не кошелёк — текст
/// причины), Internal (артефакт), узел.
/// Ревью 272 (Б2): память совпала, а запись кошелька — нет (версию сбросил откат «Записать после проверки» на тот же
/// адрес) — запись восстанавливается из памяти (иначе нативный BNB молча выключен до перезапуска).
pub async fn ensure(ctx: &Ctx, executor: Address, wallet: Address) -> GatewayResult<Version> {
    let hit = *CHECKED.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, x, w, v, wrapped)) = hit
        && (at, x, w) == (ctx.place.id, executor, wallet)
    {
        let rec = ctx.wallet().ok();
        let same = rec.is_some_and(|r| {
            r.executor_version.as_deref() == Some(v.label())
                && r.executor_wrapped == wrapped.map(|a| format!("{a:#x}"))
        });
        if !same {
            remember(ctx, executor, v, wrapped).await;
        }
        return Ok(v);
    }
    match inspect(ctx, executor, wallet).await? {
        Ok((_, v, wrapped)) => {
            *CHECKED.lock().unwrap_or_else(|p| p.into_inner()) =
                Some((ctx.place.id, executor, wallet, v, wrapped));
            remember(ctx, executor, v, wrapped).await;
            Ok(v)
        }
        Err(why) => Err(ctx.err(
            ErrorCode::InvalidAddress,
            format!(
                "исполнитель кошелька «{}»: {why} — разрешения и сделки через исполнитель нет",
                ctx.wallet_name()
            ),
        )),
    }
}

/// Решение 272: сверенная версия и (ревью А1) обёртка из хранилища v2 — у записи кошелька окружения (если адрес
/// исполнителя у него тот же). Запись не перезапускает подписки кошельков (ревью Б1, edit_version.rs). Сбой записи —
/// в журнал (сделка не останавливается: версия в памяти CHECKED уже есть). Ошибок нет.
async fn remember(ctx: &Ctx, executor: Address, v: Version, wrapped: Option<Address>) {
    let Ok(w) = ctx.wallet() else { return };
    let addr = format!("{executor:#x}");
    let label = Some(v.label().to_string());
    let wrap = wrapped.map(|a| format!("{a:#x}"));
    if let Err(e) = ctx
        .settings
        .accounts()
        .set_executor_version_at(
            ctx.place.id,
            &w.id,
            &addr,
            label,
            wrap,
            "сверка кода исполнителя",
        )
        .await
    {
        let text = format!(
            "версия исполнителя {addr} не записана у кошелька «{}»: {e}",
            w.name
        );
        ctx.log(
            crate::storage::journal::Level::Error,
            "dex.executor",
            text,
            None,
        )
        .await;
    }
}

/// Сверка с узла (без памяти). Возвращает: Ok((описание, версия, обёртка из слота wnative — только v2)) или Err(точная
/// причина). Ошибки: Internal (артефакт), узел.
async fn inspect(
    ctx: &Ctx,
    executor: Address,
    wallet: Address,
) -> GatewayResult<Result<(String, Version, Option<Address>), String>> {
    let internal = |w: String| ctx.err(ErrorCode::Internal, w);
    let have: Bytes = rpc::call(ctx, "eth_getCode", json!([executor, "latest"])).await?;
    if have.is_empty() {
        return Ok(Err(format!("по адресу {executor:#x} нет кода контракта")));
    }
    let Some(version) = code::version_of(&have).map_err(internal)? else {
        // Решение 441 (ревью, п. 7): текущая сборка contracts/out/ — v4 после пересборки (до неё побайтно v3), v3 —
        // замороженная копия contracts/out/v3/.
        let (cur, v3, v2, v1) = (
            code::runtime().map_err(internal)?,
            code::runtime_v3().map_err(internal)?,
            code::runtime_v2().map_err(internal)?,
            code::runtime_v1().map_err(internal)?,
        );
        return Ok(Err(format!(
            "код по адресу {executor:#x} ({} байт) не совпал ни с текущей сборкой contracts/out/ArbExecutor.bin-runtime \
             ({} байт), ни с v3 contracts/out/v3/ArbExecutor.bin-runtime ({} байт), ни с v2 \
             contracts/out/v2/ArbExecutor.bin-runtime ({} байт), ни с v1 contracts/out/v1/ArbExecutor.bin-runtime ({} \
             байт)",
            have.len(),
            cur.len(),
            v3.len(),
            v2.len(),
            v1.len()
        )));
    };
    let owner = Address::from_word(word(ctx, executor, code::owner_slot()).await?);
    if owner != wallet {
        return Ok(Err(format!(
            "владелец исполнителя {owner:#x} — не кошелёк бота {wallet:#x}"
        )));
    }
    // Слот wnative у v2 и v3 один и тот же (слот 1: хранилище v3 не менялось — решение 395).
    let wrapped = match version {
        Version::V2 | Version::V3 | Version::V4 => Some(Address::from_word(
            word(ctx, executor, code::wnative_slot()).await?,
        )),
        Version::V1 => None,
    };
    Ok(Ok((
        describe(ctx, executor, wallet, version, wrapped),
        version,
        wrapped,
    )))
}

/// Слово хранилища исполнителя (eth_getStorageAt) по слоту из раскладки. Ошибки: Internal (раскладка), узел.
async fn word(ctx: &Ctx, executor: Address, slot: Result<B256, String>) -> GatewayResult<B256> {
    let slot = slot.map_err(|w| ctx.err(ErrorCode::Internal, w))?;
    rpc::call(ctx, "eth_getStorageAt", json!([executor, slot, "latest"])).await
}

/// Текст успешной сверки с версией (решение 272) и обёрткой v2 против dex.native_wrapped (ревью А1). Ошибок нет.
fn describe(
    ctx: &Ctx,
    executor: Address,
    wallet: Address,
    v: Version,
    wrapped: Option<Address>,
) -> String {
    let what = match (v, wrapped, ctx.addr("dex.native_wrapped").ok()) {
        (Version::V1, _, _) => {
            "версия v1 (нативный BNB через форки и виды «только исполнитель» не торгуется — переразверните исполнитель)"
                .to_string()
        }
        (_, Some(have), Some(want)) if have == want => format!(
            "версия {}, обёртка {have:#x} = dex.native_wrapped (нативный BNB монетой сделки идёт){}",
            v.label(),
            v3_note(v)
        ),
        (_, have, want) => format!(
            "версия {}, но обёртка в исполнителе {} ≠ dex.native_wrapped {} — нативный BNB через этот исполнитель НЕ \
             идёт (ERC-20 — как обычно; переразверните исполнитель при верной dex.native_wrapped){}",
            v.label(),
            have.map_or("не прочитана".into(), |a| format!("{a:#x}")),
            want.map_or("(не задана)".into(), |a| format!("{a:#x}")),
            v3_note(v)
        ),
    };
    format!(
        "исполнитель {executor:#x}: код совпал с репозиторием, {what}, владелец — кошелёк бота {wallet:#x}"
    )
}

/// Решение 395 (t181): хвост текста сверки про шаги пулов внутри менеджеров (Uniswap V4, PancakeSwap Infinity).
/// Решение 399 (Н2): у v1 / v2 «что делать» — deploy_gate::need_v3 (пока v3 не принят — без призыва переразвернуть).
/// Ошибок нет.
fn v3_note(v: Version) -> String {
    if v >= Version::V3 {
        // Решение 400: шаги есть в коде, исполняются только классы, принятые записью симуляции на живых пулах.
        format!(
            "; шаги Uniswap V4 и PancakeSwap Infinity (CL, Bin, Stable) в коде есть — {}",
            live_proof::listing_for(v)
        )
    } else {
        format!(
            "; шагов Uniswap V4 и PancakeSwap Infinity нет (маршрут, где они смешаны с другими DEX, этим кошельком не \
             исполняется — {})",
            deploy_gate::need_v3()
        )
    }
}

/// Кнопка «Проверить подключение» исполнителя: «Подключено …» или точная причина (адрес не задан, ключ кошелька,
/// нет кода, код не тот, владелец не кошелёк, узел). Ошибок нет: причина — в details.
pub async fn probe(ctx: &Ctx) -> ConnectionCheck {
    let res = async {
        let Some(executor) = super::address(ctx).map_err(|e| e.message)? else {
            return Err(format!(
                "у кошелька «{}» не развёрнут исполнитель (экран «Маппинг кошельков»)",
                ctx.wallet_name()
            ));
        };
        let wallet = signer::address(ctx).map_err(|e| e.message)?;
        let (text, v, wrapped) = inspect(ctx, executor, wallet)
            .await
            .map_err(|e| e.message)??;
        remember(ctx, executor, v, wrapped).await;
        Ok(text)
    }
    .await;
    match res {
        Ok(s) => ConnectionCheck {
            ok: true,
            details: format!("Подключено: {s}"),
        },
        Err(why) => ConnectionCheck {
            ok: false,
            details: why,
        },
    }
}
