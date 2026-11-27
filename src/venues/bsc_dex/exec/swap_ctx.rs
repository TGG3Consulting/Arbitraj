//! Файл: venues/bsc_dex/exec/swap_ctx.rs
//! Что делает: решение 231, дополнение (Тигран, 08.11: «своп DEX по квитанции — восстанавливать итог полностью») —
//! контекст разбора итога свопа в «чем найти» (PendingRef.parse, gateway/pending.rs). Итог свопа строится в пути
//! исполнения из того, чего в квитанции нет: кошелёк, токены входа и выхода, decimals, роутеры нативной BNB (решение 62),
//! адрес исполнителя (его возврат остатка, решение 122 X), пометка «продажа по квотеру» (решение 177), подпись
//! исполнителя (решение 121 B7), причина «лучший из двух» (решение 123), чаевые и каналы транзакции. Путь кладёт это в
//! ошибку «исход неизвестен» (attach), позднее выяснение (resolve.rs → exec/swap_rebuild.rs) разбирает квитанцию теми же
//! функциями (exec/result.rs, executor/fact.rs) и даёт тот же TradeResult; решение 222 (баланс до/после) — venue.rs.
//! Разрешение (approve) с неизвестным исходом — отметка approve: обмен после него не отправлялся.
//! Ошибки с другим кодом или без факта транзакции не трогаются. Контекста нет (wallet пуст и не approve) — выясняет
//! человек (операции до дополнения, KyberSwap — см. resolve.rs). Бандл (решение 233) — тот же контекст плюс поле
//! bundle (exec/bundle_ref.rs): выясняет resolve_bundle.rs по квитанциям всех транзакций. Цикл DEX ↔ DEX (решение
//! 233, дополнение, п. 4) — тот же контекст («базовая → базовая») плюс поле cycle (курсы сделки, exec/cycle_ref.rs).
//! Хеши отмен одиночной транзакции (поле cancels; txs.rs, result.rs, swap_send.rs — with_cancels) — позднему выяснению
//! (resolve_single.rs): чья квитанция заняла nonce.
//! Кто вызывает: exec/txs.rs (seed — чаевые), exec/swap_send.rs (approve), exec/trade_direct.rs, exec/trade_smart.rs,
//! exec/executor/trade.rs, exec/bundle_trade.rs, exec/cycle_exec.rs, exec/cycle_bundle_trade.rs (attach), exec/trade.rs,
//! exec/cycle_trade.rs (подпись), best_of/trade.rs (причина), exec/swap_rebuild.rs, resolve.rs, resolve_cycle.rs.
//! От чего зависит: gateway/pending.rs, gateway/errors.rs, core/types (ChainTx).
//! Настройки: нет.

use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::core::types::ChainTx;
use crate::gateway::errors::{ErrorCode, GatewayError};
use crate::gateway::pending::PendingRef;

/// Контекст разбора итога свопа (или отметка разрешения). Все поля необязательны: по шагам их дополняют разные места.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SwapCtx {
    /// Транзакция — разрешение (approve): обмен после неё не отправлялся.
    #[serde(default)]
    pub approve: bool,
    /// Публичные параметры подписанной RFQT для повторного разбора квитанции.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rfq: Option<super::super::hashflow::ReceiptProof>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub native_rfq: Option<Box<super::super::native_rfq::settlement::Saved>>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub gladius: Option<Box<super::super::gladius::settlement::Saved>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub raindex: Option<Box<super::super::raindex::settlement::Saved>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) textile: Option<Box<super::super::textile::receipt::Saved>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) dexalot: Option<Box<super::super::dexalot::receipt::Saved>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) airswap: Option<Box<super::super::airswap::receipt::Saved>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) epsilon: Option<Box<super::super::epsilon::wire::Saved>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vista: Option<super::vista_fact::Proof>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub ekubo:Option<super::ekubo_v2_fact::Proof>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub mint_club:Option<super::mint_club_fact::Proof>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub carbon:Option<super::carbon_fact::Proof>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub bundles:Option<Box<super::bundles_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) shiba:Option<Box<super::shiba_payment::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub empire:Option<Box<super::empire_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub ammalgam:Option<Box<super::empire_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub aqua:Option<Box<super::aqua_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub angstrom:Option<Box<super::angstrom_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub stabull:Option<Box<super::stabull_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub integral:Option<Box<super::integral_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub prop:Option<Box<super::prop_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) aktionariat:Option<Box<super::aktionariat_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) bancor_legacy:Option<Box<super::bancor_legacy_wallet::Proof>>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub llamma:Option<Box<super::llamma_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) fluid_lite:Option<Box<super::fluid_lite_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub(in crate::venues::bsc_dex) ambient:Option<Box<super::ambient_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub motoswap:Option<Box<super::motoswap_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub mooniswap:Option<Box<super::mooniswap_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub defiplaza:Option<Box<super::defiplaza_wallet::Proof>>,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub yield_basis:Option<Box<super::yield_basis_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub balancer_v1:Option<Box<super::balancer_v1_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub bancor_v3:Option<Box<super::bancor_v3_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub uniswap_v1:Option<Box<super::uniswap_v1_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub rubicon:Option<Box<super::rubicon_wallet::Proof>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_flat:Option<Box<super::mars_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub dagrow:Option<Box<super::dagrow_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub temple:Option<Box<super::temple_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub pendle:Option<Box<super::pendle_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub safeswap:Option<Box<super::safeswap_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub o1:Option<Box<super::o1_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub flux_eth:Option<Box<super::flux_eth::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub antfarm:Option<Box<super::antfarm_wallet::Proof>>,
    #[serde(default)]
    pub antfarm_output:Option<Box<super::antfarm_output_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub ranges:Option<Box<super::ranges_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub registered_v3:Option<Box<super::registered_v3_wallet::Proof>>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub solidly_router:Option<Box<super::solidly_router_wallet::Proof>>,
    /// Кошелёк сделки. None — контекста свопа нет.
    #[serde(default)]
    pub wallet: Option<Address>,
    /// Токен входа и токен выхода свопа.
    #[serde(default)]
    pub tokens: Option<(Address, Address)>,
    /// Decimals монеты и USDT.
    #[serde(default)]
    pub decs: Option<(u8, u8)>,
    /// Роутеры нативной BNB (монеты — по обёртке / развороту WBNB ими); пусто — монета не нативная.
    #[serde(default)]
    pub native_routers: Vec<Address>,
    /// Адрес своего исполнителя (его возврат остатка входа — executor/fact.rs); None — не исполнитель.
    #[serde(default)]
    pub executor: Option<Address>,
    /// Ревью 272 (Б4): вызов исполнителя — executeNative (монета сделки — нативный BNB, факт по NativeSettled).
    #[serde(default)]
    pub executor_native: bool,
    /// Пометка исполнения («— продажа по квотеру», решение 177).
    #[serde(default)]
    pub note: Option<String>,
    /// Подпись исполнителя («свой сборщик: …», решение 121 B7).
    #[serde(default)]
    pub who: Option<String>,
    /// Причина выбора «лучший из двух» (решение 123).
    #[serde(default)]
    pub best_of: Option<String>,
    /// Чаевые, отправленные с транзакцией (BNB).
    #[serde(default)]
    pub tip_bnb: Option<Decimal>,
    /// Факт транзакции на момент ошибки (каналы, способ отправки, хеш, nonce).
    #[serde(default)]
    pub chain: Option<ChainTx>,
    /// Бандл (решение 233): хеши всех транзакций, первый nonce, maxBlockNumber; None — одиночная транзакция.
    #[serde(default)]
    pub bundle: Option<super::bundle_ref::BundleRef>,
    /// Цикл DEX ↔ DEX (решение 233, дополнение, п. 4): курсы на момент сделки — итог цикла при позднем выяснении
    /// (exec/cycle_ref.rs); None — не цикл.
    #[serde(default)]
    pub cycle: Option<super::cycle_ref::CycleRef>,
    /// Хеши отмен, отправленных трекером одиночной транзакции (send/track.rs, тот же nonce): Some — список известен
    /// (пустой — отмен не было); None — неизвестен («чем найти» до этого поля, бандл). Позднее выяснение
    /// (resolve_single.rs): квитанция отмены в блоке — исходная не исполнится; nonce занят ни ею, ни отменой —
    /// противоречие (только при Some).
    #[serde(default)]
    pub cancels: Option<Vec<B256>>,
}

impl SwapCtx {
    pub(in crate::venues::bsc_dex) fn module_order(&self)->Option<super::signed_call::Erc20Call> {
        self.shiba.as_deref().map(super::shiba_payment::Proof::signed)
            .or_else(|| self.aqua.as_deref().map(super::aqua_wallet::Proof::signed))
            .or_else(|| self.aktionariat.as_deref().map(super::aktionariat_wallet::Proof::signed))
            .or_else(|| self.empire.as_deref().or(self.ammalgam.as_deref()).map(super::empire_wallet::Proof::signed))
    }
    /// Контекст из «чем найти». Принимает: PendingRef. Возвращает: None — parse нет или не разобран. Ошибок нет.
    pub fn of(p: &PendingRef) -> Option<Self> {
        serde_json::from_value(p.parse.clone()?).ok()
    }

    /// Хватает ли контекста, чтобы восстановить итог: разрешение — факт транзакции; своп — кошелёк, токены, decimals и
    /// факт транзакции. Ошибок нет.
    pub fn complete(&self) -> bool {
        self.chain.is_some()
            && (self.approve
                || (self.wallet.is_some() && self.tokens.is_some() && self.decs.is_some()))
    }

    /// Контекст свопа пути исполнения. Принимает: кошелёк, (вход, выход), decimals, роутеры нативной BNB, исполнитель,
    /// пометку «по квотеру». Ошибок нет.
    pub fn swap(
        wallet: Address,
        tokens: (Address, Address),
        decs: (u8, u8),
        native_routers: &[Address],
        executor: Option<Address>,
        note: Option<String>,
    ) -> Self {
        Self {
            wallet: Some(wallet),
            tokens: Some(tokens),
            decs: Some(decs),
            native_routers: native_routers.to_vec(),
            executor,
            note,
            ..Self::default()
        }
    }

    /// Прикладывает контекст к ошибке «исход неизвестен» с фактом транзакции (чаевые и хеши отмен из прежнего «чем
    /// найти» — txs.rs seed — сохраняются). Принимает: ошибку. Возвращает: ту же ошибку. Ошибок нет.
    pub fn attach(&self, e: GatewayError) -> GatewayError {
        let mine = self.clone();
        merge(e, move |c| {
            let (tip, cancels) = (c.tip_bnb, c.cancels.take());
            *c = mine;
            c.tip_bnb = tip;
            c.cancels = cancels;
        })
    }
}

/// «Чем найти» транзакции с неизвестным исходом: хеш, nonce и чаевые (txs.rs — до того, как путь приложит контекст).
/// Принимает: факт транзакции, чаевые. Ошибок нет.
pub fn seed(chain: &ChainTx, tip_bnb: Option<Decimal>) -> PendingRef {
    let c = SwapCtx {
        tip_bnb,
        chain: Some(chain.clone()),
        ..SwapCtx::default()
    };
    PendingRef {
        venue_ref: Some(chain.hash.to_string()),
        nonce: Some(chain.nonce),
        parse: serde_json::to_value(c).ok(),
        ..PendingRef::default()
    }
}

/// «Чем найти» с хешами отправленных отмен (список известен — Some, даже пустой). Принимает: «чем найти» (seed),
/// хеши отмен. Возвращает: тот же PendingRef с полем cancels в контексте. Ошибок нет.
pub fn with_cancels(mut p: PendingRef, cancels: &[B256]) -> PendingRef {
    let mut c = SwapCtx::of(&p).unwrap_or_default();
    c.cancels = Some(cancels.to_vec());
    p.parse = serde_json::to_value(c).ok();
    p
}

/// Отметка «разрешение (approve)»: обмен после этой транзакции не отправлялся. Ошибок нет.
pub fn approve(e: GatewayError) -> GatewayError {
    merge(e, |c| c.approve = true)
}

/// Подпись исполнителя (решение 121 B7) — та же, что exec/result.rs by ставит обычному итогу. Ошибок нет.
pub fn who(e: GatewayError, who: &str) -> GatewayError {
    merge(e, |c| c.who = Some(who.to_string()))
}

/// Причина выбора «лучший из двух» (решение 123, best_of/trade.rs tag). Ошибок нет.
pub fn best_of(e: GatewayError, why: &str) -> GatewayError {
    merge(e, |c| c.best_of = Some(why.to_string()))
}

/// Меняет контекст в «чем найти» ошибки «исход неизвестен» с фактом транзакции: прежний контекст (или пустой) →
/// правка → обратно; хеш и nonce — из факта транзакции, факт — в контекст. Другие ошибки — без изменений. Ошибок нет.
fn merge(mut e: GatewayError, f: impl FnOnce(&mut SwapCtx)) -> GatewayError {
    if e.code != ErrorCode::OutcomeUnknown {
        return e;
    }
    let Some(tx) = e.tx.as_deref().cloned() else {
        return e;
    };
    let mut c = e
        .pending
        .as_deref()
        .and_then(SwapCtx::of)
        .unwrap_or_default();
    f(&mut c);
    c.chain = Some(tx.clone());
    let mut p = seed(&tx, c.tip_bnb);
    p.parse = serde_json::to_value(c).ok();
    e.pending = Some(Box::new(p));
    e
}
