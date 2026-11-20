// Файл: contracts/evmtest/world.mjs
// Что делает: общее окружение проверок EVM-теста ArbExecutor (решение 272, ревью 272): артефакты (contracts/out/ и
//   out/v1/, официальные PancakePair / PancakeFactory / ERC20 и WETH9, фикстуры fixtures/), цепь с владельцем, WBNB,
//   USDT, монетой COIN, парами Pancake USDT/WBNB, COIN/WBNB, COIN/USDT и пулом-фикстурой StableSwap BNB/COIN, исполнитель
//   (v2 — с адресом обёртки в конструкторе, ревью А1; v1 — без аргументов), кодировщики шагов и вызовов, разбор события
//   NativeSettled, выход пары V2 по формуле PancakePair.
// Кто вызывает: contracts/evmtest/run.mjs, contracts/evmtest/review.mjs.
// От чего зависит: lib.mjs, contracts/*, vendor/*, fixtures/*.
// Настройки: нет.

import { join } from 'node:path';
import { Interface } from 'ethers';
import * as L from './lib.mjs';

const { coder, must, view } = L;
export const E18 = 10n ** 18n;
export const OWNER = '0x' + 'a1'.repeat(20);
export const LP = '0x' + 'b2'.repeat(20);
export const OTHER = '0x' + 'c3'.repeat(20);
export const FAR = 2n ** 40n;

export const opt = { optimizer: { enabled: true, runs: 200 }, evmVersion: 'cancun' };
// Решение 457: исполнитель v4 (contracts/out/) собирается конвейером via-IR (settings.viaIR —
// https://docs.soliditylang.org/en/v0.8.37/using-the-compiler.html) — как в contracts/build.mjs; фикстуры — `opt`.
export const execOpt = { ...opt, viaIR: true };
const big = { optimizer: { enabled: true, runs: 999999 } };
export const v2 = L.artifact('out');
export const v1 = L.artifact('out/v1');
// Решение 395 (t181): `v2` выше — текущая сборка contracts/out/ (с решения 395 это исполнитель v3); `v2f` — замороженные
// артефакты v2 (contracts/out/v2/, до шагов UNI_V4 / INF_CL / INF_BIN) — с ними бот сверяет уже развёрнутые v2.
export const v2f = L.artifact('out/v2');
// Решение 457: замороженные артефакты v3 (contracts/out/v3/) — текущая сборка out/ с этого решения v4 (via-IR).
export const v3f = L.artifact('out/v3');
const pc = join(L.here, 'vendor', 'pancake-swap-core');
const fx = join(L.here, 'fixtures');
export const ERC20 = L.compile(L.solc05, pc, 'test/ERC20.sol', 'ERC20', big);
export const FACT = L.compile(L.solc05, pc, 'PancakeFactory.sol', 'PancakeFactory', big);
export const PAIR = L.compile(L.solc05, pc, 'PancakePair.sol', 'PancakePair', big);
export const WETH = L.compile(L.solc06, join(L.here, 'vendor', 'v2-periphery'), 'WETH9.sol', 'WETH9', big);
// Фикстуры (кода в репозитории нет — fixtures/*.sol, шапки файлов): пул StableSwap с BNB, токен с повторным входом,
// контракт-владелец, пул V3 с режимами обмана, поддельная обёртка.
export const STB = L.compile(L.solc08, fx, 'StableBnbFixture.sol', 'StableBnbFixture', opt);
export const RE = L.compile(L.solc08, fx, 'ReentrantToken.sol', 'ReentrantToken', opt);
export const OWN = L.compile(L.solc08, fx, 'OwnerProxy.sol', 'OwnerProxy', opt);
export const EVIL = L.compile(L.solc08, fx, 'EvilV3Pool.sol', 'EvilV3Pool', opt);
export const FAKE = L.compile(L.solc08, fx, 'FakeWrapper.sol', 'FakeWrapper', opt);
export const X = new Interface(v2.abi);

/// Пара Pancake V2 токенов a/b с ликвидностью (wa, wb) от LP; токены уже у LP. Возвращает адрес пары.
export async function pairOf(c, f, a, b, wa, wb, abiA = ERC20.abi, abiB = ERC20.abi) {
  await must(c, LP, f, FACT.abi, 'createPair', [a, b]);
  const pair = await view(c, f, FACT.abi, 'getPair', [a, b]);
  await must(c, LP, a, abiA, 'transfer', [pair, wa]);
  await must(c, LP, b, abiB, 'transfer', [pair, wb]);
  await must(c, LP, pair, PAIR.abi, 'mint', [LP]);
  return pair;
}

/// Исполнитель от имени from: v2 — конструктор(wnative), v1 — без аргументов. Возвращает адрес.
export async function executor(c, from, art, wnative) {
  return art === v1 ? L.deploy(c, from, art) : L.deploy(c, from, art, ['address'], [wnative]);
}

/// Цепь с владельцем, WBNB, USDT, COIN, парами (USDT/WBNB 60000:100, COIN/WBNB 1000:100, COIN/USDT 1000:60000),
/// пулом-фикстурой BNB/COIN и исполнителем версии art.
export async function world(art = v2) {
  const c = await L.chain();
  for (const a of [OWNER, LP, OTHER]) await L.fund(c, a, 100_000n * E18);
  const w = await L.deploy(c, LP, WETH);
  const usdt = await L.deploy(c, LP, ERC20, ['uint256'], [10n ** 30n]);
  const coin = await L.deploy(c, LP, ERC20, ['uint256'], [10n ** 30n]);
  const f = await L.deploy(c, LP, FACT, ['address'], [LP]);
  await must(c, LP, w, WETH.abi, 'deposit', [], 1_000n * E18);
  const pair = await pairOf(c, f, usdt, w, 60_000n * E18, 100n * E18, ERC20.abi, WETH.abi);
  const pairCW = await pairOf(c, f, coin, w, 1_000n * E18, 100n * E18, ERC20.abi, WETH.abi);
  const pairCU = await pairOf(c, f, coin, usdt, 1_000n * E18, 60_000n * E18);
  const stable = await L.deploy(c, LP, STB, ['address', 'uint256'], [coin, 2300n], 50n * E18);
  await must(c, LP, coin, ERC20.abi, 'transfer', [stable, 50n * E18]);
  await must(c, LP, usdt, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10n * E18]);
  const ex = await executor(c, OWNER, art, w);
  return { c, f, w, usdt, coin, pair, pairCW, pairCU, stable, ex };
}

export const v2Step = (pool, tin, tout, bps = 10000) => [0, pool, tin, tout, bps, coder.encode(['uint32'], [2500])];
export const v3Step = (pool, tin, tout, bps = 10000) => [1, pool, tin, tout, bps, '0x'];
export const stStep = (pool, tin, tout, i, j, bnbIn) => [11, pool, tin, tout, 10000, coder.encode(['uint256', 'uint256', 'bool'], [i, j, bnbIn])];
/// Данные executeNative (ревью А1: обёртки в параметрах нет — она из конструктора).
export const nat = (wrapIn, tin, amt, tout, min, steps, deadline = FAR) =>
  X.encodeFunctionData('executeNative', [wrapIn, tin, amt, tout, min, deadline, steps]);
export const exe = (tin, amt, tout, min, steps, deadline = FAR) =>
  X.encodeFunctionData('execute', [tin, amt, tout, min, deadline, steps]);
/// NativeSettled(nativeIn, nativeOut) исполнителя ex в итоге вызова; null — события нет.
export const settled = (r, ex) => {
  const t = X.getEvent('NativeSettled').topicHash;
  const l = r.logs.find((x) => x.address.toLowerCase() === ex.toLowerCase() && x.topics[0] === t);
  return l ? coder.decode(['uint256', 'uint256'], l.data).map(BigInt) : null;
};
export const bal = (c, t, a) => view(c, t, ERC20.abi, 'balanceOf', [a]).then(BigInt);

/// Выход пары Pancake V2 (комиссия 25/10000 — PancakePair.sol L180-L181) при текущих резервах.
export async function v2Out(c, pair, tin, amt) {
  const res = await L.call(c, LP, pair, new Interface(PAIR.abi).encodeFunctionData('getReserves'));
  const [a, b] = new Interface(PAIR.abi).decodeFunctionResult('getReserves', res.ret).map(BigInt);
  const t0 = await view(c, pair, PAIR.abi, 'token0');
  const [ri, ro] = t0.toLowerCase() === tin.toLowerCase() ? [a, b] : [b, a];
  const inFee = amt * 9975n;
  return (inFee * ro) / (ri * 10000n + inFee);
}
