//! Узкая проверка чтения ERC20 balanceOf по опубликованному runtime. Вместо
//! содержимого наблюдаемого слова используется символ. Допускается только
//! возврат этого целого символа. Маска, арифметика, ветвление по его значению,
//! запись хранилища и посторонние внешние вызовы завершают проверку отказом.
use super::presell_layout::{ADMIN_SLOT, IMPL_SLOT};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
mod machine;
use machine::{Machine, Word};

pub(super) struct Code {
    pub account: Address,
    pub bytecode: Bytes,
}
pub(super) struct Read<'a> {
    pub token: &'a Code,
    pub implementation: Option<&'a Code>,
    pub admin: Option<B256>,
    pub caller: Address,
    pub holder: Address,
    pub key: B256,
    pub storage: Address,
}
impl Read<'_> {
    pub(super) fn proves(&self) -> bool {
        let mut data = [0u8; 36];
        data[..4].copy_from_slice(&[0x70, 0xa0, 0x82, 0x31]);
        data[4..].copy_from_slice(self.holder.into_word().as_slice());
        self.body(self.token, &data, self.token.account, self.caller, true) == Some(())
    }
    fn body(
        &self,
        code: &Code,
        data: &[u8; 36],
        storage: Address,
        caller: Address,
        outer: bool,
    ) -> Option<()> {
        let bytes = code.bytecode.as_ref();
        let mut m = Machine::default();
        let mut pc = 0usize;
        let mut got_return = false;
        let mut seen = false;
        // Простая ABI-диспетчеризация и полный getter имеют конечный короткий путь.
        // Ограничиваем только проверяемый язык, не объём продажи.
        for _ in 0..bytes.len().saturating_mul(2) {
            let op = *bytes.get(pc)?;
            pc += 1;
            match op {
                0x01..=0x04 | 0x06 | 0x10..=0x1d => m.arithmetic(op)?,
                0x20 => {
                    let off = m.index()?;
                    let size = m.index()?;
                    let raw = m.memory(off, size)?;
                    let word = keccak256(raw);
                    let value = U256::from_be_bytes(word.0);
                    let derived = size == 64
                        && raw
                            .chunks_exact(32)
                            .any(|w| w == self.holder.into_word().as_slice());
                    m.push(if derived {
                        Word::Key(value)
                    } else {
                        Word::Plain(value)
                    })?;
                }
                0x30 => m.num(U256::from_be_slice(storage.as_slice()))?,
                0x33 => m.num(U256::from_be_slice(caller.as_slice()))?,
                0x34 | 0x5f => m.num(U256::ZERO)?,
                0x35 => {
                    let off = m.index()?;
                    let mut out = [0; 32];
                    if let Some(tail) = data.get(off..) {
                        let n = tail.len().min(32);
                        out[..n].copy_from_slice(&tail[..n]);
                    }
                    m.num(U256::from_be_bytes(out))?;
                }
                0x36 => m.num(U256::from(data.len()))?,
                0x37 => {
                    let dest = m.index()?;
                    let at = m.index()?;
                    let len = m.index()?;
                    m.copy(dest, data, at, len)?;
                }
                0x38 => m.num(U256::from(bytes.len()))?,
                0x39 => {
                    let dest = m.index()?;
                    let at = m.index()?;
                    let len = m.index()?;
                    m.copy(dest, bytes, at, len)?;
                }
                0x3b => {
                    let address = m.pop()?.value()?;
                    let imp = self.implementation?;
                    if address != U256::from_be_slice(imp.account.as_slice()) {
                        return None;
                    }
                    m.num(U256::from(imp.bytecode.len()))?;
                }
                0x3d => m.num(U256::from(if got_return { 32u8 } else { 0u8 }))?,
                0x3e => {
                    let dest = m.index()?;
                    let at = m.index()?;
                    let n = m.index()?;
                    if !(got_return && at == 0 && n == 32) {
                        return None;
                    }
                    m.store(dest, Word::Balance)?;
                }
                0x50 => {
                    m.pop()?;
                }
                0x51 => {
                    let off = m.index()?;
                    let v = m.load(off)?;
                    m.push(v)?;
                }
                0x52 => {
                    let off = m.index()?;
                    let v = m.pop()?;
                    m.store(off, v)?;
                }
                0x54 => {
                    let operand = m.pop()?;
                    let key = B256::from(operand.value()?.to_be_bytes::<32>());
                    if storage == self.storage
                        && matches!(operand, Word::Key(_))
                        && key == self.key
                        && !seen
                    {
                        seen = true;
                        m.push(Word::Balance)?;
                    } else if outer && key == IMPL_SLOT {
                        let imp = self.implementation?;
                        m.num(U256::from_be_slice(imp.account.as_slice()))?;
                    } else if outer && key == ADMIN_SLOT {
                        m.num(U256::from_be_bytes(self.admin?.0))?;
                    } else {
                        return None;
                    }
                }
                0x56 | 0x57 => {
                    let dest = m.index()?;
                    let take = op == 0x56 || !m.pop()?.value()?.is_zero();
                    if take {
                        if bytes.get(dest) != Some(&0x5b) {
                            return None;
                        }
                        pc = dest;
                    }
                }
                0x5a => {
                    // GAS разрешён только как первый аргумент штатного DELEGATECALL.
                    if !matches!(bytes.get(pc).copied(), Some(0xf4 | 0xfa)) {
                        return None;
                    }
                    m.num(U256::MAX)?;
                }
                0x5b => {}
                0x60..=0x7f => {
                    let n = usize::from(op - 0x5f);
                    m.num(U256::from_be_slice(bytes.get(pc..pc.checked_add(n)?)?))?;
                    pc += n;
                }
                0x80..=0x8f => m.duplicate(usize::from(op - 0x7f))?,
                0x90..=0x9f => m.swap(usize::from(op - 0x8f))?,
                0xf3 => {
                    let at = m.index()?;
                    let n = m.index()?;
                    return (n == 32
                        && matches!(m.load(at)?, Word::Balance)
                        && (seen || got_return))
                        .then_some(());
                }
                0xf4 | 0xfa => {
                    if !outer || seen || got_return {
                        return None;
                    }
                    let imp = self.implementation?;
                    m.pop()?.value()?;
                    if m.pop()?.value()? != U256::from_be_slice(imp.account.as_slice()) {
                        return None;
                    }
                    let at = m.index()?;
                    let n = m.index()?;
                    let out = m.index()?;
                    let out_len = m.index()?;
                    if m.memory(at, n)? != data || !matches!(out_len, 0 | 32) {
                        return None;
                    }
                    let (inner_storage, sender) = if op == 0xf4 {
                        (storage, caller)
                    } else {
                        (imp.account, storage)
                    };
                    self.body(imp, data, inner_storage, sender, false)?;
                    if out_len == 32 {
                        m.store(out, Word::Balance)?;
                    }
                    got_return = true;
                    m.num(U256::from(1))?;
                }
                _ => return None,
            }
        }
        None
    }
}
