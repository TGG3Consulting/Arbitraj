//! Файл: tests/core_net_chain_489_held.rs
//! Что делает: решение 489 — сеть выключенной площадки DEX держится, пока сделка не завершена полностью, включая
//! возврат USDT (поток — по решению 196, сеть — core/net_hold.rs), на записанных ответах BingX 2026-11-21
//! (tests/data/bingx_2026-11-21) и работающем ядре (t489_common; сделки — настоящим ledger::save, справочник —
//! напрямую, book.rs built):
//! (1) arb_dex не включена, открытая сделка через неё сохранена — сеть захвачена (Inner.net_held); записи ARBITRUM с
//!     сетью arbitrum и адресом (dirctx находит точность и шаг вывода — net_ready::withdraw пропускает), VELA в своей
//!     сети, присутствие биржи с данными сети (подписка биржи) и флагами записи; направлений нет — problem
//!     «… только идущие сделки (решение 196)», отказ исполнителя (start_reason) с этой причиной; USDT сети известен;
//! (2) сделка закрыта, возврат USDT с arb_dex идёт («идёт», затем «ждёт повтора») — сеть держится, USDT сети есть;
//! (3) возврат закончен — сеть отпущена (памяти нет), повторные сохранения закрытой сделки — без толчков (net_hold
//!     on_saved = false), адрес ARBITRUM снимается, монет arbitrum нет, USDT сети нет;
//! (4) зеркальный случай: bsc_dex выключена и держится сделкой, arb_dex включена — без утечки.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain_play, ledger, net_hold, net_ready, rebuild_build, start_reason), t489_common.
//! Моков нет.

mod t489_common;

use alloy_primitives::address;
use serde_json::json;

use arb::core::chain_play::in_play;
use arb::core::types::Direction;
use arb::settings::scope::Scope;
use t489_common::book::{built, cex_set, chained, dex_set, pick, usdt_other};
use t489_common::deals::{closed_with, edit, hold, net_held, part};
use t489_common::{NEW, arb_chain, arb_only, feed, raw, start, v};

/// Причина «сеть держат только сделки» (rebuild_build.rs held_only).
const HELD: &str = "только идущие сделки (решение 196)";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_network_until_usdt_return_done() {
    let core = start("net-chain-489-held", true, false).await;
    let a = arb_chain();
    assert!(!net_held(&core, "arb_dex"));
    hold(&core, "H489", "arb_dex", Some(a.clone())).await;
    assert!(net_held(&core, "arb_dex"), "захват (ledger::save)");
    assert!(in_play(&core, &v("arb_dex")), "держит открытая сделка");
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    let list = feed(&core, NEW, raw.clone());
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.chain, Some(a.clone()));
    let vela_addr = address!("088cd8f5ef3652623c22d48b1605dcfe860cd704");
    assert_eq!(rec.asset.contract, Some(vela_addr));
    let all = built(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    let held = pick(&all, Some(&a), &only);
    assert!(held.len() > 40, "монеты держимой сети в справочнике");
    let vela = held.iter().find(|c| c.asset.ticker == "VELA").unwrap();
    let ready = arb::core::net_ready::withdraw(&core, &v("bingx"), &vela.asset, "ARBITRUM");
    assert!(ready.is_ok(), "{ready:?}");
    let p = vela
        .presence
        .iter()
        .find(|p| p.venue == v("bingx"))
        .unwrap();
    let info = p.info.as_ref().unwrap();
    assert_eq!(info.network, "ARBITRUM");
    assert_eq!(p.can_send, rec.withdraw_enabled);
    assert_eq!(p.can_receive, rec.deposit_enabled);
    let on_cex = cex_set(&held, "bingx");
    assert_eq!(on_cex.len(), held.len(), "подписка биржи на монеты сети");
    for c in &held {
        let why = c.problem.as_deref().unwrap_or("");
        assert!(why.contains(HELD), "{}: {why}", c.asset.ticker);
        assert!(c.directions().is_empty());
    }
    let d = Direction {
        asset: vela.asset.clone(),
        buy_on: v("bingx"),
        sell_on: v("arb_dex"),
    };
    let why = arb::core::start_reason::not_tradable(Some(vela), &d);
    assert!(why.contains(HELD), "{why}");
    assert!(usdt_other(&core).contains_key(&a), "USDT сети arbitrum");
    let sub = dex_set(&core, &all, "bsc_dex");
    assert_eq!(chained(&sub), 0, "набор bsc_dex без монет arbitrum");

    // Сделка закрыта, возврат выручки USDT с arb_dex на bingx идёт — сеть держится.
    closed_with(&core, "H489", part("pending", "arb_dex", "bingx")).await;
    assert!(net_held(&core, "arb_dex"), "возврат идёт — держится");
    closed_with(&core, "H489", part("retrying", "arb_dex", "bingx")).await;
    assert!(net_held(&core, "arb_dex"), "ждёт повтора — держится");
    assert!(in_play(&core, &v("arb_dex")), "держит возврат USDT");
    feed(&core, NEW, raw.clone());
    let all = built(&core);
    assert!(usdt_other(&core).contains_key(&a), "USDT для возврата");
    let held = pick(&all, Some(&a), &only);
    assert!(held.len() > 40, "монеты в своей сети");
    let none = held.iter().all(|c| c.directions().is_empty());
    assert!(none, "новых сделок нет");

    // Возврат закончен — сеть отпущена; повторные сохранения — без толчков.
    closed_with(&core, "H489", part("done", "arb_dex", "bingx")).await;
    assert!(!net_held(&core, "arb_dex"), "отпущена (ledger::save)");
    assert!(!in_play(&core, &v("arb_dex")));
    edit(&core, "H489", |_| {}).await;
    let rec = core
        .inner
        .deals
        .lock()
        .unwrap()
        .get("H489")
        .cloned()
        .unwrap();
    let kick = arb::core::net_hold::on_saved(&core, &rec);
    assert!(!kick, "повтор — без толчка");
    assert!(!net_held(&core, "arb_dex"));
    let list = feed(&core, NEW, raw);
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.contract, None, "адрес снят");
    let all = built(&core);
    let bsc = all.iter().filter(|c| c.asset.contract.is_some()).count();
    assert!(bsc >= 20, "монеты BSC собраны");
    assert_eq!(chained(&all), 0, "сети arbitrum нет");
    assert!(pick(&all, None, &only).is_empty());
    assert!(!usdt_other(&core).contains_key(&a), "USDT не известен");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_held_bsc_dex_no_leak() {
    let core = start("net-chain-489-mirror", true, true).await;
    let off = Scope::Venue("bsc_dex".into());
    t489_common::set(&core, "venue.enabled", off, json!(false)).await;
    hold(&core, "M489", "bsc_dex", None).await;
    assert!(in_play(&core, &v("bsc_dex")));
    let a = arb_chain();
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    feed(&core, NEW, raw);
    let all = built(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    assert!(pick(&all, Some(&a), &only).len() > 40, "монеты arbitrum");
    for c in all.iter().filter(|c| c.asset.chain.is_none()) {
        let why = c.problem.as_deref().unwrap_or("");
        assert!(!why.contains(HELD), "{}: {why}", c.asset.ticker);
    }
    let sub = dex_set(&core, &all, "arb_dex");
    assert!(!sub.is_empty(), "набор arb_dex есть");
    assert!(sub.iter().all(|c| c.asset.chain == Some(a.clone())));
}

/// Реальное хранилище настроек и ядро: любую из четырёх сетей можно
/// выключить без смены сети старых записей. Учитывает как карту in_play,
/// так и группы, поступающие в сборщик рабочего справочника монет.
#[tokio::test]
async fn four_networks_have_sixteen_independent_activation_states() {
    use arb::core::{chain, chain_play, registry_chains};
    use arb::gateway::contract::VenueKind;
    let core = start("base-eth-four-networks-live-flags", true, false).await;
    let venues = [
        ("bsc_dex", "bsc"),
        ("arb_dex", "arbitrum"),
        ("base_dex", "base"),
        ("eth_dex", "ethereum"),
    ];
    for (name, network) in &venues[2..] {
        let id = v(name);
        core.inner.kinds.write().unwrap().push((id, VenueKind::Dex));
        t489_common::set(
            &core,
            "dex.chain",
            Scope::Venue((*name).into()),
            json!(network),
        )
        .await;
    }
    for mask in 0_u8..16 {
        for (i, (name, _)) in venues.iter().enumerate() {
            t489_common::set(
                &core,
                "venue.enabled",
                Scope::Venue((*name).into()),
                json!(mask & (1 << i) != 0),
            )
            .await;
        }
        let expected: Vec<_> = venues
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, (name, _))| v(name))
            .collect();
        let active = chain_play::active_dexes(&core);
        assert_eq!(active, expected, "маска {mask:04b}");
        let groups = registry_chains::groups(&core, &chain::dexes(&core));
        let registered: Vec<_> = groups.into_iter().flat_map(|g| g.dex).collect();
        assert_eq!(registered, expected, "группы {mask:04b}");
        for (name, network) in &venues {
            let chain = chain::tag_of_dex(&core, &v(name)).unwrap().map(|c| c.0);
            assert_eq!(chain.as_deref(), (*network != "bsc").then_some(*network));
        }
    }
}

/// Один маппинг кошелька для двух физических сетей. Stop одной отделён от второй,
/// одинаковый адрес токена не объединяет деньги; незакрытые старые планы восстанавливают исходный DEX.
#[tokio::test]
async fn cycles_share_mapping_but_keep_network_stop_locks_and_recovery_separate() {
    use arb::core::types::{ChainId, DexCycle};
    use arb::core::{
        chain, chain_run,
        coin_key::CoinKey,
        coin_ride, cycle_plan,
        deal::{Deal, Step},
        dex_account, ledger, persist,
    };
    use arb::gateway::contract::VenueKind;
    use arb::settings::accounts::ROW_DEX_DEX;
    use rust_decimal::Decimal;

    let core = start("cycles-network-stop", true, false).await;
    let accounts = core.inner.deps.settings.accounts();
    accounts
        .add("same", "Общий", "ref", "test-queue", "тест")
        .await
        .unwrap();
    accounts
        .map_set(ROW_DEX_DEX, Some("same"), "тест")
        .await
        .unwrap();
    let h: serde_json::Value =
        serde_json::from_str(include_str!("data/base_ethereum_networks.json")).unwrap();
    for (name, val) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.min_bnb_balance", "0.01"),
        ("risk.max_daily_loss_usd", "15"),
    ] {
        t489_common::set(&core, name, Scope::Global, json!(val)).await;
    }
    let mut dirs = Vec::new();
    for (name, network) in [("base_dex", "base"), ("eth_dex", "ethereum")] {
        core.inner
            .kinds
            .write()
            .unwrap()
            .push((v(name), VenueKind::Dex));
        t489_common::set(
            &core,
            "dex.chain",
            Scope::Venue(name.into()),
            json!(network),
        )
        .await;
        for (key, value) in [
            ("venue.enabled", json!(true)),
            ("dex_dex.execute", json!(true)),
            ("risk.max_errors_in_row", json!(3)),
        ] {
            t489_common::set(&core, key, Scope::Venue(name.into()), value).await;
        }
        let cycle: DexCycle = serde_json::from_value(json!({
            "venue": name, "block": h[network]["block"], "base": "0x4200000000000000000000000000000000000006",
            "base_ticker":"BASE","input":"1","output":"1","gross_profit":"0","gas_units":0,
            "gas_bnb":"0","tip_bnb":"0","bnb_cost":"0","hops":[],"executable":true
        })).unwrap();
        let direction = cycle_plan::direction(&core, &cycle).unwrap();
        assert_eq!(direction.asset.chain, Some(ChainId(network.into())));
        assert_eq!(
            dex_account::of_direction_now(&core, &direction).unwrap().0,
            "same"
        );
        let mut old = direction.clone();
        old.asset.chain = None;
        assert_eq!(chain::of_dir(&core, &old).unwrap().0, network);
        let mut deal = Deal::new(name.into(), old, json!({"cycle":cycle}), Decimal::ZERO);
        deal.step = Step::Halted;
        deal.dex_account = dex_account::of_direction_now(&core, &direction);
        persist::save_deal(&core.inner.deps.db, &deal)
            .await
            .unwrap();
        dirs.push(direction);
    }
    ledger::load(&core).await;
    for d in &dirs {
        let restored = core
            .deals()
            .into_iter()
            .find(|x| x.id == d.buy_on.0)
            .unwrap();
        assert_eq!(restored.direction, *d);
        assert_eq!(restored.step, Step::Halted);
        assert_eq!(restored.dex_account.unwrap().0, "same");
        assert!(coin_ride::take(&core, &CoinKey::of(&d.asset)));
        assert!(!coin_ride::take(&core, &CoinKey::of(&d.asset)));
    }
    for (on, off) in [(0, 1), (1, 0)] {
        let a = dirs[on].asset.chain.as_ref().unwrap();
        let b = dirs[off].asset.chain.as_ref().unwrap();
        chain_run::set(&core, &a.0, true, "тест").unwrap();
        chain_run::set(&core, &b.0, false, "тест").unwrap();
        let checked = |d: &Direction| {
            let c = chain_run::gate_input(&core, d).unwrap();
            chain_run::refusal(&core.inner.risk.lock().unwrap(), c)
        };
        assert!(checked(&dirs[on]).is_none());
        assert!(checked(&dirs[off]).unwrap().contains(&b.0));
        // Вызов собран при включённой сети; пока он ждёт узел, оператор остановил именно её.
        // На том же задании первый POST уже подтверждён, поэтому его учёт вне запрета новой отправки.
        let guard_core = core.clone();
        let guard_dir = dirs[on].clone();
        let guard = move || {
            arb::core::cycle_deal::submission_ok(
                &guard_core,
                &guard_dir,
                &arb::core::types::DexAccountId("same".into()),
            )
            .map_err(|why| {
                arb::gateway::errors::GatewayError::new(
                    arb::gateway::errors::ErrorCode::NotConfigured,
                    None,
                    why,
                )
            })
        };
        arb::gateway::cycle_guard::scope(guard, async {
            assert!(arb::gateway::cycle_guard::before_post().is_ok());
            chain_run::set(&core, &a.0, false, "во время подготовки").unwrap();
            assert!(arb::gateway::cycle_guard::before_post().is_err());
        })
        .await;
        assert!(arb::gateway::cycle_guard::before_post().is_ok());
    }
}
