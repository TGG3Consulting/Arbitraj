//! Файл: venues/bsc_dex/math/ekubo_exp2.rs
//! Что делает: exp2 Ekubo (решение 454) — 2^x, x — число 5.64, итог — 64.64; точный порт (те же 64 множителя и сдвиги).
//! Нужна расчёту цены после виртуальных ордеров TWAMM (math/ekubo_twamm.rs, computeNextSqrtRatio).
//! Кто вызывает: math/ekubo_twamm.rs.
//! От чего зависит: alloy-primitives.
//! Настройки: нет (множители — константы протокола, не параметры).
//!
//! Источник — EkuboProtocol/evm-contracts@61d09daa src/math/exp2.sol L5-L205 (множитель бита 63 − i — FACTORS[i];
//! итог >> (63 − целая часть x)). Множители перенесены из исходника скриптом (строка в строку), не набраны вручную.

use alloy_primitives::U256;

/// Множители по битам дробной части x, от бита 63 к биту 0 (exp2.sol: `result * C >> 128`).
const FACTORS: [&str; 64] = [
    "16A09E667F3BCC908B2FB1366EA957D3E",
    "1306FE0A31B7152DE8D5A46305C85EDEC",
    "1172B83C7D517ADCDF7C8C50EB14A791F",
    "10B5586CF9890F6298B92B71842A98363",
    "1059B0D31585743AE7C548EB68CA417FD",
    "102C9A3E778060EE6F7CACA4F7A29BDE8",
    "10163DA9FB33356D84A66AE336DCDFA3F",
    "100B1AFA5ABCBED6129AB13EC11DC9543",
    "10058C86DA1C09EA1FF19D294CF2F679B",
    "1002C605E2E8CEC506D21BFC89A23A00F",
    "100162F3904051FA128BCA9C55C31E5DF",
    "1000B175EFFDC76BA38E31671CA939725",
    "100058BA01FB9F96D6CACD4B180917C3D",
    "10002C5CC37DA9491D0985C348C68E7B3",
    "1000162E525EE054754457D5995292026",
    "10000B17255775C040618BF4A4ADE83FC",
    "1000058B91B5BC9AE2EED81E9B7D4CFAB",
    "100002C5C89D5EC6CA4D7C8ACC017B7C9",
    "10000162E43F4F831060E02D839A9D16D",
    "100000B1721BCFC99D9F890EA06911763",
    "10000058B90CF1E6D97F9CA14DBCC1628",
    "1000002C5C863B73F016468F6BAC5CA2B",
    "100000162E430E5A18F6119E3C02282A5",
    "1000000B1721835514B86E6D96EFD1BFE",
    "100000058B90C0B48C6BE5DF846C5B2EF",
    "10000002C5C8601CC6B9E94213C72737A",
    "1000000162E42FFF037DF38AA2B219F06",
    "10000000B17217FBA9C739AA5819F44F9",
    "1000000058B90BFCDEE5ACD3C1CEDC823",
    "100000002C5C85FE31F35A6A30DA1BE50",
    "10000000162E42FF0999CE3541B9FFFCF",
    "100000000B17217F80F4EF5AADDA45554",
    "10000000058B90BFBF8479BD5A81B51AD",
    "1000000002C5C85FDF84BD62AE30A74CC",
    "100000000162E42FEFB2FED257559BDAA",
    "1000000000B17217F7D5A7716BBA4A9AE",
    "100000000058B90BFBE9DDBAC5E109CCE",
    "10000000002C5C85FDF4B15DE6F17EB0D",
    "1000000000162E42FEFA494F1478FDE05",
    "10000000000B17217F7D20CF927C8E94C",
    "1000000000058B90BFBE8F71CB4E4B33D",
    "100000000002C5C85FDF477B662B26945",
    "10000000000162E42FEFA3AE53369388C",
    "100000000000B17217F7D1D351A389D40",
    "10000000000058B90BFBE8E8B2D3D4EDE",
    "1000000000002C5C85FDF4741BEA6E77E",
    "100000000000162E42FEFA39FE95583C2",
    "1000000000000B17217F7D1CFB72B45E1",
    "100000000000058B90BFBE8E7CC35C3F0",
    "10000000000002C5C85FDF473E242EA38",
    "1000000000000162E42FEFA39F02B772C",
    "10000000000000B17217F7D1CF7D83C1A",
    "1000000000000058B90BFBE8E7BDCBE2E",
    "100000000000002C5C85FDF473DEA871F",
    "10000000000000162E42FEFA39EF44D91",
    "100000000000000B17217F7D1CF79E949",
    "10000000000000058B90BFBE8E7BCE544",
    "1000000000000002C5C85FDF473DE6ECA",
    "100000000000000162E42FEFA39EF366F",
    "1000000000000000B17217F7D1CF79AFA",
    "100000000000000058B90BFBE8E7BCD6D",
    "10000000000000002C5C85FDF473DE6B2",
    "1000000000000000162E42FEFA39EF358",
    "10000000000000000B17217F7D1CF79AB",
];

/// Предел x: 0x400000000000000000 (exp2.sol L7: require — иначе откат).
fn limit() -> U256 {
    U256::from(1u8) << 70usize
}

/// 2^x. Принимает: x (5.64, < 2^70). Возвращает: 64.64. None — x не меньше предела (откат в контракте). Ошибок нет.
pub fn exp2(x: U256) -> Option<U256> {
    if x >= limit() {
        return None;
    }
    let mut result = U256::from(1u8) << 127usize;
    for (i, c) in FACTORS.iter().enumerate() {
        if x.bit(63 - i) {
            let f = U256::from_str_radix(c, 16).unwrap_or_default();
            result = (result * f) >> 128usize;
        }
    }
    let int = (x >> 64usize).to::<u64>();
    Some(result >> (63 - int as usize))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256;

    use super::exp2;

    fn n(s: &str) -> U256 {
        U256::from_str_radix(s, 10).unwrap()
    }

    /// Векторы официального теста: evm-contracts@e5574ab test/math/exp2.t.sol L32-L47 (exp2.sol не менялся с 61d09daa).
    #[test]
    fn official_vectors() {
        let one = U256::from(1u8);
        assert_eq!(exp2(one << 62usize), Some(n("21936999301089678046")));
        assert_eq!(exp2(one << 63usize), Some(n("26087635650665564424")));
        assert_eq!(exp2(U256::ZERO), Some(one << 64usize));
        assert_eq!(exp2(one << 64usize), Some(U256::from(2u8) << 64usize));
        let x = (U256::from(3u8) << 64usize) / U256::from(2u8);
        assert_eq!(exp2(x), Some(n("52175271301331128849")));
        assert_eq!(exp2(U256::from(16u8) << 64usize), Some(U256::from(65536u32) << 64usize));
        let y = (U256::from(127u8) << 64usize) / U256::from(2u8);
        assert_eq!(exp2(y), Some(n("240615969168004511545033772477625056927")));
        let max = (one << 70usize) - one;
        assert_eq!(exp2(max), Some(n("340282366920938463450588298786565555714")));
        assert_eq!(exp2(one << 70usize), None);
    }
}
