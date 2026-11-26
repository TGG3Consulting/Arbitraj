//! Ethereum Ring Swap. Канонические underlying и Few wrappers приняты по FewFactory,
//! обратным token()/factory(), decimals и CREATE2 Ring factory на блоке 26494988.
//! Порядок резервов совпадает с физическими fw-адресами, а торгуются underlying.
//! Контракты и ABI: https://ringprotocol.gitbook.io/ring/docs/deployments.
//! Изолированное воспроизведение: scripts/record_ring_ethereum.py.

pub use super::ring_acceptance::checked;
pub use super::ring_profile::{Layout, active};
use alloy::sol_types::SolValue;
use alloy_primitives::{Address, Bytes, U256};
impl Layout {
    /// FewFactory.createToken: salt=keccak256(abi.encode(originalToken)), не abi.encodePacked.
    pub(super) fn wrapped(&self, asset: Address) -> Address {
        self.few.create2(
            alloy_primitives::keccak256(asset.into_word()),
            self.wrapper_init,
        )
    }

    /// Обе стороны — underlying. Регистрация подтверждает фабрики, байткод,
    /// обратные связи, decimals и резервы; исполнимую сумму проверяет финальная котировка.
    pub fn path(&self, a: Address, b: Address) -> Option<[Address; 2]> {
        (a != b && !a.is_zero() && !b.is_zero()).then(|| [self.wrapped(a), self.wrapped(b)])
    }

    pub fn check_pool(&self, pool: Address, fw0: Address, fw1: Address) -> bool {
        super::verify::v2_create2(self.factory, fw0, fw1, self.pair_init) == pool
    }

    /// FEW_V2_SWAP_EXACT_IN возвращает underlying получателю через unwrapTo.
    /// Последующую проверку реального баланса добавляет штатный Plan::data.
    pub fn exact_input(
        &self,
        token_in: Address,
        token_out: Address,
        recipient: Address,
        amount: U256,
        minimum: U256,
        payer_user: bool,
    ) -> Option<(u8, Bytes)> {
        let path = self.path(token_in, token_out)?.to_vec();
        if !self.direct.contains(&token_in) {
            // У остальных входов используем условное разрешение. USDT запрещает
            // approve(nonzero) поверх прежнего разрешения. Команда 0x27
            // использует существующий allowance UR → fwUSDT, как Payments.wrapFewToken.
            let mut commands = Vec::new();
            let mut inputs = Vec::new();
            if payer_user {
                if amount.bit_len() > 160 {
                    return None;
                }
                commands.push(0x02u8);
                inputs.push(Bytes::from(
                    (token_in, Address::with_last_byte(2), amount).abi_encode_params(),
                ));
            }
            let pool = super::verify::v2_create2(self.factory, path[0], path[1], self.pair_init);
            commands.extend([0x27, 0x23]);
            inputs.push(Bytes::from((token_in, pool, amount).abi_encode_params()));
            inputs.push(Bytes::from(
                (recipient, U256::ZERO, minimum, path, false, true).abi_encode_params(),
            ));
            return Some((
                0x21,
                Bytes::from((Bytes::from(commands), inputs).abi_encode_params()),
            ));
        }
        Some((
            0x23,
            Bytes::from((recipient, amount, minimum, path, payer_user, true).abi_encode_params()),
        ))
    }
}

#[cfg(test)]
#[path = "../../../../tests/common/first_run_profiles.rs"]
mod first_run_profiles;
#[cfg(test)]
pub(super) use first_run_profiles::named as archived_profile;

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> (Layout, [(Address, Address); 5], serde_json::Value) {
        let raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/first_run_dex_2026-11-26/ring_ethereum.json"
        ))
        .unwrap();
        let d =
            super::archived_profile(raw["chain_id"].as_u64().unwrap(), "ring-exchange-ethereum")
                .unwrap();
        let layout = Layout::archived(d).unwrap();
        let pools = raw["pools"].as_array().unwrap();
        let assets: Vec<_> = pools.iter().map(|p| p["asset"].as_str().unwrap()).collect();
        let wrappers = raw["wrappers"].as_object().unwrap();
        let wrapped_native = wrappers
            .keys()
            .find(|asset| !assets.contains(&asset.as_str()))
            .unwrap()
            .as_str();
        let a: Vec<_> = std::iter::once(wrapped_native)
            .chain(assets)
            .map(|k| {
                (
                    k.parse().unwrap(),
                    wrappers[k].as_str().unwrap().parse().unwrap(),
                )
            })
            .collect();
        (layout, a.try_into().unwrap(), raw)
    }
    #[test]
    fn wrapped_reserves_keep_their_physical_order_and_approval_uses_underlying() {
        let (layout, assets, raw) = inputs();
        let [
            (eth, w_eth),
            (usdt, w_usdt),
            (usdc, w_usdc),
            (wbtc, _),
            (uni, _),
        ] = assets;
        let usdc_pool = raw["pools"][1]["pool"]
            .as_str()
            .unwrap()
            .parse::<Address>()
            .unwrap();
        assert!(layout.check_pool(usdc_pool, w_usdc, w_eth));
        assert_eq!(layout.wrapped(usdc), w_usdc);
        assert_eq!(layout.wrapped(eth), w_eth);
        assert_eq!(layout.path(eth, usdt), Some([w_eth, w_usdt]));
        assert_eq!(layout.path(Address::ZERO, usdt), None);
        assert_eq!(
            layout.path(wbtc, uni),
            Some([layout.wrapped(wbtc), layout.wrapped(uni)])
        );
        let (command, data) = layout
            .exact_input(eth, usdt, eth, U256::from(7), U256::from(4), true)
            .unwrap();
        let args =
            <(Address, U256, U256, Vec<Address>, bool, bool)>::abi_decode_params(&data).unwrap();
        assert_eq!(command, 0x23);
        assert_eq!(
            args,
            (
                eth,
                U256::from(7),
                U256::from(4),
                vec![w_eth, w_usdt],
                true,
                true
            )
        );
        let (nested, bytes) = layout
            .exact_input(usdt, eth, eth, U256::from(7), U256::from(4), true)
            .unwrap();
        let (commands, inputs) = <(Bytes, Vec<Bytes>)>::abi_decode_params(&bytes).unwrap();
        assert_eq!(nested, 0x21);
        assert_eq!(commands.as_ref(), &[0x02, 0x27, 0x23]);
        let (asset, pool, amount) = <(Address, Address, U256)>::abi_decode_params(&inputs[1]).unwrap();
        assert_eq!(
            (asset, pool, amount),
            (
                usdt,
                raw["pools"][0]["pool"]
                    .as_str()
                    .unwrap()
                    .parse::<Address>()
                    .unwrap(),
                U256::from(7)
            )
        );
        let args = <(Address, U256, U256, Vec<Address>, bool, bool)>::abi_decode_params(&inputs[2])
            .unwrap();
        assert_eq!(
            args,
            (
                eth,
                U256::ZERO,
                U256::from(4),
                vec![w_usdt, w_eth],
                false,
                true
            )
        );
        assert_eq!(super::super::exec_kind::of("ring_few_v2"), None);
    }
    #[test]
    fn archived_underlying_and_native_roundtrips_match_both_reserve_directions() {
        let (layout, assets, j) = inputs();
        assert_eq!(j["chain_id"], 1);
        assert_eq!(j["block"], 26_494_988);
        assert_eq!(j["selector"]["requireCanonical"], true);
        let raw = |v: &serde_json::Value| v.as_str().unwrap().parse::<U256>().unwrap();
        let price = |a: U256, ri: U256, ro: U256| {
            let net = a * U256::from(997);
            net * ro / (ri * U256::from(1000) + net)
        };
        let (weth, fw_eth) = assets[0];
        let pools = j["pools"].as_array().unwrap();
        assert_eq!(pools.len(), assets.len() - 1);
        for (row, (asset, fw)) in pools.iter().zip(&assets[1..]) {
            assert_eq!(row["asset"], format!("{asset:#x}"));
            assert_eq!(j["wrappers"][format!("{asset:#x}")], format!("{fw:#x}"));
            assert_eq!(layout.path(weth, *asset), Some([fw_eth, *fw]));
            let pool = row["pool"].as_str().unwrap().parse().unwrap();
            assert!(layout.check_pool(pool, fw_eth, *fw));
            let ri = raw(&row["reserve_weth"]);
            let ro = raw(&row["reserve_other"]);
            let cases = row["cases"].as_array().unwrap();
            assert_eq!(cases.len(), 4);
            for (i, c) in cases.iter().enumerate() {
                let amount = raw(&c["in"]);
                let out = price(amount, ri, ro);
                let back = price(out, ro - out, ri + amount);
                assert_eq!(c["native"], i >= 2);
                assert_eq!(amount, U256::from(10u64.pow(15 + (i % 2) as u32)));
                assert_eq!(out, raw(&c["out"]));
                assert_eq!(back, raw(&c["roundtrip"]));
                let balances = &c["balances"];
                assert_eq!(raw(&balances[1][1]), out);
                assert_eq!(raw(&balances[2][1]), U256::ZERO);
                assert_eq!(raw(&balances[2][0]), if i >= 2 { U256::ZERO } else { back });
                assert!(
                    c["gas"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|v| v.as_u64().unwrap() > 21_000)
                );
                let owner = j["owner"].as_str().unwrap().parse().unwrap();
                let deadline = U256::from(j["deadline"].as_u64().unwrap());
                let native = i >= 2;
                for (d, (source, target, input, minimum)) in
                    [(weth, *asset, amount, out), (*asset, weth, out, back)]
                        .into_iter()
                        .enumerate()
                {
                    use crate::venues::bsc_dex::exec::ur;
                    let mut steps = Vec::new();
                    if native && d == 0 {
                        steps.push((ur::WRAP_ETH, ur::native(ur::ADDRESS_THIS, input)));
                    }
                    let recipient = if native && d == 1 {
                        ur::ADDRESS_THIS
                    } else {
                        owner
                    };
                    steps.push(
                        layout
                            .exact_input(
                                source,
                                target,
                                recipient,
                                input,
                                minimum,
                                !(native && d == 0),
                            )
                            .unwrap(),
                    );
                    if native && d == 1 {
                        steps.push((ur::UNWRAP_WETH, ur::native(owner, minimum)));
                    } else {
                        steps.push((
                            ur::BALANCE_CHECK_ERC20,
                            ur::balance_check(owner, target, minimum),
                        ));
                    }
                    let hash = alloy_primitives::keccak256(ur::execute(steps, deadline));
                    assert_eq!(c["swap_calldata_keccak"][d], format!("{hash:#x}"));
                }
                if native {
                    assert_eq!(raw(&c["native_received"]), back);
                }
                let controls = &c["checks"];
                for name in [
                    "nominal_plus_one0",
                    "nominal_plus_one1",
                    "balance_plus_one0",
                ] {
                    assert_eq!(controls[name], "0x0");
                }
                if i < 2 {
                    assert_eq!(controls["balance_plus_one1"], "0x0");
                }
                if *asset == assets[1].0 {
                    assert_eq!(controls["legacy_usdt_nonzero_approve"], "0x0");
                }
            }
        }
    }
}
