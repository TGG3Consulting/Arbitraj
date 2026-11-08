//! Файл: venues/bsc_dex/exec/single_rebuild.rs
//! Что делает: позднее выяснение ОДИНОЧНОЙ транзакции кошелька (своп, разрешение, цикл через исполнитель) с неизвестным
//! исходом, когда в «чем найти» известны хеши отправленных трекером отмен (exec/swap_ctx.rs cancels; ветка
//! t42/small-fixes, дополнение решений 231 / 233 п. 4). Чистая логика без сети (сеть — resolve_single.rs).
//! (1) judge — по тому, что видит свой узел (квитанция исходной, квитанции отмен, знает ли узел их в блоке, nonce
//! кошелька на блоке «голова − send.replaced_confirm_blocks», прочитанный ДО квитанций):
//! - квитанция исходной → «в блоке» (итог — exec/swap_rebuild.rs, как раньше);
//! - квитанция отмены → «не исполнено»: nonce занят нашей отменой, исходная с этим nonce уже не исполнится
//!   (send/cancel.rs: перевод 0 BNB себе тем же nonce);
//! - обе квитанции → «противоречие» (один nonce — одна транзакция в цепи);
//! - квитанций нет, узел знает транзакцию в блоке → «ждём» (квитанция появится);
//! - квитанций нет, а nonce занят не позже блока «голова − send.replaced_confirm_blocks» → «противоречие»: место заняла
//!   не наша транзакция и не наша отмена — исход проверяет человек;
//! - иначе — «ждём».
//!
//! (2) cancelled — итог «не исполнено» тем же видом, что обычный путь даёт при отмене, занявшей nonce (exec/result.rs:
//! TxNotIncluded «nonce занят отменой», факт транзакции с cancel_hash и cancel_gas_bnb — газ отмены в затраты ядра,
//! решение 229 cancel_confirmed), с подписью исполнителя и причиной «лучший из двух», как обычный путь.
//!
//! Кто вызывает: resolve_single.rs; тесты (tests/bsc_dex_single_cancel.rs).
//! От чего зависит: exec/{chain_tx, result, swap_ctx, swap_rebuild}.rs, send/{receipt, track_util}.rs, core/types.
//! Настройки: нет (send.replaced_confirm_blocks читает resolve_single.rs и передаёт в Seen).
//!
//! Квитанция: null, пока нет в блоке — execution-apis 16caee27 src/eth/transaction.yaml#L127-L165;
//! eth_getTransactionCount(address, block) — src/eth/state.yaml.

use alloy_primitives::B256;

use crate::core::types::{TradeResult, TxOutcome};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::ctx::Ctx;
use super::super::send::receipt::Receipt;
use super::super::send::track_util::Tracked;
use super::swap_ctx::SwapCtx;
use super::{chain_tx, result, swap_rebuild};

/// Что видит свой узел по одиночной транзакции и её отменам.
pub struct Seen {
    /// Квитанция исходной транзакции (None — нет в блоке).
    pub ours: Option<Receipt>,
    /// Первая найденная квитанция отмены: хеш и квитанция (None — ни одной).
    pub cancel: Option<(B256, Receipt)>,
    /// Узел знает исходную или отмену в блоке (eth_getTransactionByHash), а квитанции ещё нет.
    pub in_block: bool,
    /// Nonce транзакции.
    pub nonce: u64,
    /// eth_getTransactionCount(кошелёк, голова − confirm) — прочитан ДО квитанций.
    pub nonce_deep: u64,
    /// Номер свежей головы.
    pub head: u64,
    /// send.replaced_confirm_blocks.
    pub confirm: u64,
}

/// Вердикт по одиночной транзакции.
#[derive(Debug)]
pub enum Verdict {
    /// Исход пока не установлен (причина — для журнала).
    Wait(String),
    /// Исходная транзакция в блоке.
    Ours(Receipt),
    /// Nonce занят нашей отменой в блоке: исходная не исполнится.
    Cancelled(B256, Receipt),
    /// Противоречие — человек (причина).
    Odd(String),
}

/// Что делает: вердикт (см. шапку). Принимает: что видит узел, сколько отмен в «чем найти». Возвращает: вердикт.
/// Ошибок нет.
pub fn judge(s: Seen, cancels: usize) -> Verdict {
    match (s.ours, s.cancel) {
        (Some(_), Some((h, _))) => Verdict::Odd(format!(
            "в блоке и исходная транзакция, и отмена {h} с тем же nonce {}",
            s.nonce
        )),
        (Some(r), None) => Verdict::Ours(r),
        (None, Some((h, r))) => Verdict::Cancelled(h, r),
        (None, None) if s.in_block => {
            Verdict::Wait("узел знает транзакцию в блоке, квитанции ещё нет".into())
        }
        (None, None) if s.nonce_deep > s.nonce => Verdict::Odd(format!(
            "nonce {} занят не позже блока {}, а квитанций нет ни у транзакции, ни у её отмен ({cancels}) — место \
             заняла не наша транзакция",
            s.nonce,
            s.head.saturating_sub(s.confirm)
        )),
        (None, None) => Verdict::Wait(format!(
            "квитанций нет, nonce {} не занят на блоке {}",
            s.nonce,
            s.head.saturating_sub(s.confirm)
        )),
    }
}

/// Что делает: итог «не исполнено» — nonce занят нашей отменой в блоке. Тот же вид, что у обычного пути
/// (exec/result.rs: TxNotIncluded с фактом транзакции, cancel_hash / cancel_gas_bnb), подпись исполнителя и причина
/// «лучший из двух» — как обычный путь. Принимает: окружение, контекст из «чем найти» (chain обязателен), хеш исходной,
/// отмену (хеш, квитанция), строку об очереди кошелька. Возвращает: ошибку TxNotIncluded (всегда Err).
/// Ошибки: Internal — в контексте нет факта транзакции.
pub fn cancelled(
    ctx: &Ctx,
    c: &SwapCtx,
    hash: B256,
    (h, r): (B256, Receipt),
    queue: &str,
) -> GatewayResult<TradeResult> {
    let Some(tmpl) = c.chain.as_ref() else {
        return Err(ctx.err(ErrorCode::Internal, "контекст разбора без факта транзакции"));
    };
    let block = r.block_number;
    let t = Tracked {
        outcome: TxOutcome::NotIncluded,
        receipt: None,
        cancel: Some((h, r)),
        unresolved: None,
    };
    let chain = chain_tx::chain_tx(
        hash,
        tmpl.nonce,
        tmpl.channels.clone(),
        tmpl.method,
        &t,
        c.tip_bnb,
    );
    let e: GatewayError = ctx
        .err(
            ErrorCode::TxNotIncluded,
            format!(
                "Транзакция не попала в блок, tx {hash} — nonce занят отменой {h} (блок {block}; выяснено ботом по \
                 квитанции отмены); {queue}"
            ),
        )
        .with_tx(chain);
    let out = match &c.who {
        Some(w) => result::by(Err(e), w),
        None => Err(e),
    };
    swap_rebuild::tail(out, c.best_of.as_deref())
}
