//! Рабочая память исключительно для доказательства ERC20 getter. Символ Balance
//! не имеет числового значения. Частичное копирование, вычисления или ветвления
//! по его байтам отклоняются; средства записи в состояние EVM отсутствуют.
//! Семантика стека, памяти и арифметики — приложение H Ethereum Yellow Paper.
//! <https://ethereum.github.io/yellowpaper/paper.pdf>
//! Опкоды: https://ethereum.github.io/yellowpaper/paper.pdf, приложение H.
use alloy_primitives::U256;
#[derive(Clone, Copy)]
pub(super) enum Word {
    Plain(U256),
    Key(U256),
    Balance(U256),
}
impl Word {
    pub(super) fn value(self) -> Option<U256> {
        match self {
            Self::Plain(n) | Self::Key(n) => Some(n),
            Self::Balance(_) => None,
        }
    }
}
#[derive(Default)]
pub(super) struct Machine {
    stack: Vec<Word>,
    data: Vec<u8>,
    symbols: Vec<u8>,
}
impl Machine {
    pub(super) fn push(&mut self, w: Word) -> Option<()> {
        if self.stack.len() >= 1024 {
            return None;
        }
        self.stack.push(w);
        Some(())
    }
    pub(super) fn num(&mut self, n: U256) -> Option<()> {
        self.push(Word::Plain(n))
    }
    pub(super) fn pop(&mut self) -> Option<Word> {
        self.stack.pop()
    }
    pub(super) fn index(&mut self) -> Option<usize> {
        usize::try_from(self.pop()?.value()?).ok()
    }
    pub(super) fn duplicate(&mut self, n: usize) -> Option<()> {
        self.push(*self.stack.get(self.stack.len().checked_sub(n)?)?)
    }
    pub(super) fn swap(&mut self, n: usize) -> Option<()> {
        let a = self.stack.len().checked_sub(1)?;
        let b = a.checked_sub(n)?;
        self.stack.swap(a, b);
        Some(())
    }
    fn reserve(&mut self, at: usize, n: usize) -> Option<usize> {
        let end = at.checked_add(n)?;
        // Ограничение памяти локального доказательства, иначе обычное пополнение.
        if end > 4096 {
            return None;
        }
        if self.data.len() < end {
            self.data.resize(end, 0);
            self.symbols.resize(end, 0);
        }
        Some(end)
    }
    pub(super) fn memory(&mut self, at: usize, n: usize) -> Option<&[u8]> {
        let end = self.reserve(at, n)?;
        self.symbols[at..end]
            .iter()
            .all(|v| *v == 0)
            .then_some(&self.data[at..end])
    }
    pub(super) fn load(&mut self, at: usize) -> Option<Word> {
        let end = self.reserve(at, 32)?;
        if self.symbols[at..end].iter().copied().eq(1..=32) {
            return Some(Word::Balance(U256::from_be_slice(&self.data[at..end])));
        }
        Some(Word::Plain(U256::from_be_slice(self.memory(at, 32)?)))
    }
    pub(super) fn store(&mut self, at: usize, w: Word) -> Option<()> {
        let end = self.reserve(at, 32)?;
        if let Some(v) = w.value() {
            self.symbols[at..end].fill(0);
            self.data[at..end].copy_from_slice(&v.to_be_bytes::<32>());
        } else if let Word::Balance(mask) = w {
            self.data[at..end].copy_from_slice(&mask.to_be_bytes::<32>());
            for (b, i) in self.symbols[at..end].iter_mut().zip(1..=32) {
                *b = i
            }
        }
        Some(())
    }
    pub(super) fn copy(&mut self, dest: usize, source: &[u8], at: usize, n: usize) -> Option<()> {
        let end = self.reserve(dest, n)?;
        self.symbols[dest..end].fill(0);
        self.data[dest..end].fill(0);
        if let Some(tail) = source.get(at..) {
            let available = tail.len().min(n);
            self.data[dest..dest + available].copy_from_slice(&tail[..available]);
        }
        Some(())
    }
    pub(super) fn input(&mut self, op: u8, data: &[u8; 36], bytes: &[u8]) -> Option<()> {
        match op {
            0x35 => {
                let off = self.index()?;
                let mut out = [0; 32];
                if let Some(tail) = data.get(off..) {
                    let n = tail.len().min(32);
                    out[..n].copy_from_slice(&tail[..n]);
                }
                self.num(U256::from_be_bytes(out))?;
            }
            0x36 => self.num(U256::from(data.len()))?,
            0x37 => {
                let dest = self.index()?;
                let at = self.index()?;
                let len = self.index()?;
                self.copy(dest, data, at, len)?;
            }
            0x38 => self.num(U256::from(bytes.len()))?,
            0x39 => {
                let dest = self.index()?;
                let at = self.index()?;
                let len = self.index()?;
                self.copy(dest, bytes, at, len)?;
            }
            _ => return None,
        }
        Some(())
    }
    pub(super) fn arithmetic(&mut self, op: u8) -> Option<()> {
        let a = self.pop()?;
        let b = self.stack.last().copied();
        // Solidity старых версий извлекает поле в нулевом смещении через DIV 1.
        if let (0x04, Word::Balance(_), Some(Word::Plain(divisor))) = (op, a, b)
            && divisor == U256::from(1)
        {
            self.pop()?;
            return self.push(a);
        }
        if let (0x16, Some(b)) = (op, b) {
            let mask = match (a, b) {
                (Word::Balance(prior), Word::Plain(value))
                | (Word::Plain(value), Word::Balance(prior)) => Some(prior & value),
                _ => None,
            };
            if let Some(mask) = mask {
                let width = mask.bit_len();
                if width == 0 || !width.is_multiple_of(8) || mask != U256::MAX >> (256 - width) {
                    return None;
                }
                self.pop()?;
                return self.push(Word::Balance(mask));
            }
        }
        let x = a.value()?;
        if op == 0x15 {
            return self.num(U256::from(x.is_zero() as u8));
        }
        if op == 0x19 {
            return self.num(!x);
        }
        let b = self.pop()?;
        let y = b.value()?;
        let bit = |p: bool| U256::from(p as u8);
        let sign = U256::from(1) << 255;
        let v = match op {
            0x01 => x.wrapping_add(y),
            0x02 => x.wrapping_mul(y),
            0x03 => x.wrapping_sub(y),
            0x04 => x.checked_div(y).unwrap_or_default(),
            0x06 => x.checked_rem(y).unwrap_or_default(),
            0x0a if y.is_zero() => U256::from(1),
            0x10 => bit(x < y),
            0x11 => bit(x > y),
            0x12 => bit((x ^ sign) < (y ^ sign)),
            0x13 => bit((x ^ sign) > (y ^ sign)),
            0x14 => bit(x == y),
            0x16 => x & y,
            0x17 => x | y,
            0x18 => x ^ y,
            0x1b | 0x1c if x >= U256::from(256) => U256::ZERO,
            0x1b => y << usize::try_from(x).ok()?,
            0x1c => y >> usize::try_from(x).ok()?,
            _ => return None,
        };
        if op == 0x01 && (matches!(a, Word::Key(_)) || matches!(b, Word::Key(_))) {
            self.push(Word::Key(v))
        } else {
            self.num(v)
        }
    }
}
