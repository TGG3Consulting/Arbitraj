// Файл: contracts/evmtest/ekubo.mjs
// Что делает: решение 458 (T457-01) — окружение и помощники шага EKUBO (25) исполнителя ArbExecutor v4 на EVM ethereumjs с ОФИЦИАЛЬНЫМ кодом
//   Ekubo — ровно код развёртывания в Arbitrum One: коды создания из broadcast EkuboProtocol/evm-contracts
//   (vendor/ekubo, файлы без изменений, хеши — vendor/SHA256SUMS): DeployAll.s.sol/42161/run-1767194711580.json (коммит
//   ccb3a419, сборка «commit» 61d09da — Core 0x0000…14aA…d701, MEVCapture 0x5555…41Be) и
//   DeployManagers.s.sol/42161/run-1767205373297.json (коммит 504b42f6, сборка 8a99714 — Positions 0x02d9…b66d, им
//   кладётся ликвидность). Развёртывание — как в сети: через детерминированный CREATE2-прокси
//   0x4e59b44847b379578588920ca78fbf26c0b4956c (Arachnid/deterministic-deployment-proxy@be3c5974 README «Deployment
//   Transaction»: код создания прокси) с данными транзакций broadcast (соль 32 байта + код создания) — адреса
//   сверяются с broadcast. Код собран solc 0.8.33 под Osaka (foundry.toml @61d09daa) и исполняет опкод CLZ — цепь с CLZ
//   по EIP-7939 (lib.mjs chain({ clz })). Котировка — fixtures/EkuboQuoter.sol (путь Router.quote: lock → swap / forward
//   → откат с balanceUpdate ядра).
//   K1) ERC-20 ↔ ERC-20 (USDT/COIN, концентрированный пул без расширения), обе стороны: владелец получил ровно выход
//       котировки ядра и выход записи обмена ядра (log0), списано ровно amountIn, выход — переводом ядро → владелец,
//       на исполнителе пусто; minOut + 1 — «minOut»;
//   K2) нативный вход: пул (ETH, COIN), шаг WETH → COIN с native = true — execute (WETH владельца) и executeNative
//       (продажа ETH value): ядру ушло ровно amountIn ETH (баланс ядра), получено = котировка, WETH / ETH на исполнителе 0;
//   K3) нативный выход: COIN → WETH (native) — withdraw ETH исполнителю → deposit в WETH9 → владельцу ровно котировка;
//       executeNative (покупка ETH) — владелец получил ETH = котировка;
//   K4) MEVCapture (forward = true): получено = котировка через forward (меньше выхода пула на доплату MEVCapture);
//       тот же пул без forward — откат SwapMustHappenThroughForward;
//   K5) защита: посторонний / владелец вне шага / ядро вне шага зовут locked_6416899205 — «pool»; пул не создан —
//       откат ядра; «ядро» шага — не ядро — откат; native у шага без обёртки — «wnative»; токен с налогом на входе —
//       откат всего; исполнитель v3 (out/v3) — «kind».
// Кто вызывает: contracts/evmtest/ekubo_cases.mjs (сценарии K1–K5 — там; run.mjs зовёт ekubo_cases.mjs).
// От чего зависит: lib.mjs, world.mjs, vendor/ekubo, fixtures/EkuboQuoter.sol, fixtures/TaxToken.sol.
// Настройки: нет.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { must, coder } = L;
const { E18, OWNER, LP, OTHER, ERC20, v2, world, bal } = W;
const EKUBO = 25;
const bc = (p) => JSON.parse(readFileSync(join(L.here, 'vendor', 'ekubo', 'broadcast', p), 'utf8'));
const ALL = bc('DeployAll.s.sol/42161/run-1767194711580.json');
const MAN = bc('DeployManagers.s.sol/42161/run-1767205373297.json');
const tx = (b, name) => b.transactions.find((t) => t.transactionType === 'CREATE2' && t.contractName === name);
// Код создания прокси (Arachnid/deterministic-deployment-proxy@be3c5974 README: данные транзакции развёртывания).
const PROXY_INIT = '604580600e600039806000f350fe7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf3';
const PROXY = '0x4e59b44847b379578588920ca78fbf26c0b4956c';
const QT = L.compile(L.solc08, join(L.here, 'fixtures'), 'EkuboQuoter.sol', 'EkuboQuoter', W.opt);
const TAX = L.compile(L.solc08, join(L.here, 'fixtures'), 'TaxToken.sol', 'TaxToken', W.opt);
const KEY = 'tuple(address,address,bytes32)';
const POS = new Interface([
  `function maybeInitializePool(${KEY} poolKey, int32 tick) payable returns (bool, uint96)`,
  `function mintAndDepositWithSalt(bytes32 salt, ${KEY} poolKey, int32 tickLower, int32 tickUpper, uint128 maxAmount0, uint128 maxAmount1, uint128 minLiquidity) payable returns (uint256, uint128, uint128, uint128)`,
]);
const ZERO = '0x' + '00'.repeat(20);
// Пределы цены (sqrtRatio.sol L13, L15 @61d09daa) — те же, что EKUBO_MIN_SQRT / EKUBO_MAX_SQRT исполнителя.
const MIN_SQRT = 4611797791050542631n;
const MAX_SQRT = 79227682466138141934206691491n;
const FEE = ((1n << 64n) * 3n) / 1000n; // 0,3 % в 0.64
const SPACING = 10000n;
const RANGE = 1_000_000n; // позиция [-RANGE, RANGE] (кратно шагу)

/// PoolConfig концентрированного пула (poolConfig.sol L141-L151): расширение << 96 | fee << 32 | 0x80000000 | шаг.
const config = (ext = ZERO) =>
  '0x' + ((BigInt(ext) << 96n) | (FEE << 32n) | 0x80000000n | SPACING).toString(16).padStart(64, '0');
/// SwapParameters точного входа (swapParameters.sol L42-L58): предел << 160 | сумма << 32 | isToken1 << 31.
const params = (amt, isToken1) =>
  '0x' + (((isToken1 ? MAX_SQRT : MIN_SQRT) << 160n) | (amt << 32n) | (isToken1 ? 1n << 31n : 0n)).toString(16).padStart(64, '0');
const sort = (a, b) => (BigInt(a) < BigInt(b) ? [a, b] : [b, a]);
const step = (core, tin, tout, cfg, native = false, fwd = false, bps = 10000) =>
  [EKUBO, core, tin, tout, bps, coder.encode(['bytes32', 'bool', 'bool'], [cfg, native, fwd])];
/// delta0 / delta1 из balanceUpdate (poolBalanceUpdate.sol L8-L18): старшие и младшие 128 бит со знаком.
const deltas = (bu) => {
  const v = BigInt(bu);
  const s = (x) => (x >= 1n << 127n ? x - (1n << 128n) : x);
  return [s(v >> 128n), s(v & ((1n << 128n) - 1n))];
};

/// Контракт из broadcast — через прокси CREATE2 с данными транзакции; адрес сверяется с broadcast. Возвращает адрес.
async function create2(c, t) {
  const r = await L.call(c, LP, PROXY, t.transaction.input);
  if (!r.ok) throw new Error(`${t.contractName}: развёртывание откатилось ${r.reason}`);
  const a = '0x' + r.ret.slice(2);
  if (a.toLowerCase() !== t.contractAddress.toLowerCase()) throw new Error(`${t.contractName}: адрес ${a} ≠ broadcast`);
  return a;
}

/// Мир: world.mjs (исполнитель art, цепь с CLZ) + прокси, Core, MEVCapture, Positions, пулы USDT/COIN (без
/// расширения и с MEVCapture) и (ETH, COIN) с ликвидностью, котировщик, разрешения владельца исполнителю.
async function env(art = v2) {
  const w = await world(art, { clz: true });
  const { c, usdt, coin, ex } = w;
  const p = await L.call(c, LP, null, '0x' + PROXY_INIT);
  await c.sm.putContractCode(L.addr(PROXY), await c.sm.getContractCode(L.addr(p.created)));
  const core = await create2(c, tx(ALL, 'Core'));
  const mev = await create2(c, tx(ALL, 'MEVCapture'));
  const pos = await create2(c, tx(MAN, 'Positions'));
  const [t0, t1] = sort(usdt, coin);
  for (const t of [usdt, coin]) await must(c, LP, t, ERC20.abi, 'approve', [pos, 10n ** 30n]);
  const kU = [t0, t1, config()];
  const kM = [t0, t1, config(mev)];
  const kN = [ZERO, coin, config()];
  const e = { c, pos };
  await addLiquidity(e, kU);
  await addLiquidity(e, kM);
  await addLiquidity(e, kN, 1_000n * E18);
  const qt = await L.deploy(c, LP, QT);
  for (const t of [usdt, coin]) await must(c, OWNER, t, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  return { ...w, core, mev, pos, qt, kU, kM, kN };
}

/// Ликвидность от LP через официальный Positions: maybeInitializePool(ключ, тик 0) → mintAndDepositWithSalt([-RANGE,
/// RANGE] (range — иной симметричный диапазон: полный диапазон пула TWAMM, решение 458 T458-02), по 10 000 каждого токена; нативный token0 — value). Соль у каждой позиции своя: id NFT Positions без соли
/// повторяется (откат TokenAlreadyExists при второй). Токены LP одобрены Positions заранее. Ошибки: исключение.
let salt = 0n;
async function addLiquidity(e, key, value = 0n, range = RANGE) {
  const { c, pos } = e;
  const lq = 10_000n * E18;
  const i = await L.call(c, LP, pos, POS.encodeFunctionData('maybeInitializePool', [key, 0]));
  if (!i.ok) throw new Error(`maybeInitializePool: ${i.reason} ${i.ret}`);
  salt += 1n;
  const r = await L.call(c, LP, pos, POS.encodeFunctionData('mintAndDepositWithSalt',
    ['0x' + salt.toString(16).padStart(64, '0'), key, -range, range, value || lq, lq, 0]), value);
  if (!r.ok) throw new Error(`mintAndDepositWithSalt: ${r.reason} ${r.ret}`);
}

/// Котировка ядра (fixtures/EkuboQuoter.sol) для ключа key, входа tin (адрес ключа; нативный — 0), суммы, forward.
/// Возвращает: [долг входа, выход] (выход > 0).
async function quote(e, key, tin, amt, fwd = ZERO) {
  const isToken1 = BigInt(tin) === BigInt(key[1]);
  const r = await L.call(e.c, OTHER, e.qt, new Interface(QT.abi).encodeFunctionData('quote', [e.core, key[0], key[1], key[2], params(amt, isToken1), fwd]));
  if (!r.ok) throw new Error(`quote: ${r.reason} ${r.ret}`);
  const [d0, d1] = deltas(coder.decode(['bytes32'], r.ret)[0]);
  return isToken1 ? [d1, -d0] : [d0, -d1];
}

/// Записи обмена ядра (log0 от ядра, 116 байт: locker 20 | poolId 32 | balanceUpdate 32 | stateAfter 32 — Core.sol
/// L840-L847). Возвращает: [[delta0, delta1]].
const coreSwaps = (r, core) => r.logs.filter((l) => l.address.toLowerCase() === core.toLowerCase() && l.topics.length === 0
  && (l.data.length - 2) / 2 === 116).map((l) => deltas('0x' + l.data.slice(2 + 104, 2 + 168)));

const transfers = (r, t, from, to) => {
  const T = new Interface(ERC20.abi).getEvent('Transfer').topicHash;
  return r.logs.filter((l) => l.address.toLowerCase() === t.toLowerCase() && l.topics[0] === T
    && BigInt(l.topics[1]) === BigInt(from) && BigInt(l.topics[2]) === BigInt(to));
};

const empty = async (e, ts) => {
  for (const t of ts) if ((await bal(e.c, t, e.ex)) !== 0n) return false;
  return (await L.balance(e.c, e.ex)) === 0n;
};

export { ALL, MAN, tx, create2, env, addLiquidity, quote, coreSwaps, transfers, empty, step, config, sort, deltas, ZERO, EKUBO, TAX };
