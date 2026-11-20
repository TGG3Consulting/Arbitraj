// Файл: contracts/evmtest/fluid.mjs
// Что делает: решение 458 (T457-01) — окружение шага FLUID (22) исполнителя ArbExecutor v4 с ОФИЦИАЛЬНЫМ кодом Fluid —
//   коды создания из артефактов развёртывания Arbitrum One репозитория Instadapp/fluid-contracts-public@404d73f
//   (vendor/fluid-contracts-public/deployments/arbitrum, файлы без изменений, хеши — vendor/SHA256SUMS): слой ликвидности
//   Liquidity (прокси) + LiquidityDummyImpl + AdminModule + UserModule, DexFactory (права администратора пула),
//   DexT1Admin / DexT1Shift, пул Dex_USDC_ETH (код пула Fluid DEX T1) и его ColOperations / DebtOperations /
//   PerfectOperationsAndSwapOut. Настройка — как тесты репозитория (test/foundry/liquidity/liquidityBaseTest.t.sol L51-L253,
//   liquidityTestHelpers.sol L539-L651, test/foundry/dex/poolT1/pool.t.sol L269-L380): реализации прокси, права, ставки
//   (kink 80 %, 4 / 10 / 150 %), токены (fee 0, порог 0, использование 100 %), лимиты пула в слое ликвидности (высокие),
//   initialize пула (smart col, цена центра 1e27, диапазоны 10 %, пороги сдвига 5 %, сутки, пределы цены центра ±20 %).
//   Пул разворачивается кодом создания артефакта с ConstantViews, как DexT1DeploymentLogic.dexT1 (poolT1Logic.sol
//   L80-L122: слоты слоя ликвидности — _calculateLiquidityDexSlots L152-L189; операции — тем же ConstantViews без
//   реализаций), а не через фабрику (её код создания хранится SSTORE2-указателями развёртывания).
// Кто вызывает: contracts/evmtest/fluid_cases.mjs.
// От чего зависит: lib.mjs, world.mjs, vendor/fluid-contracts-public.
// Настройки: нет.

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Interface, keccak256, getCreateAddress } from 'ethers';
import * as L from './lib.mjs';
import * as W from './world.mjs';

const { must, coder } = L;
const { E18, OWNER, LP, ERC20, v2, world } = W;
const dir = join(L.here, 'vendor', 'fluid-contracts-public', 'deployments', 'arbitrum');
/// Артефакт hardhat-deploy: { abi, bin (без 0x), args }.
const art = (n) => {
  const j = JSON.parse(readFileSync(join(dir, `${n}.json`), 'utf8'));
  return { abi: j.abi, bin: j.bytecode.slice(2), args: j.args };
};
export const A = Object.fromEntries(['Liquidity', 'LiquidityDummyImpl', 'AdminModule', 'UserModule', 'DexFactory', 'DexT1Admin',
  'DexT1Shift', 'Dex_USDC_ETH', 'Dex_USDC_ETH_ColOperations', 'Dex_USDC_ETH_DebtOperations',
  'Dex_USDC_ETH_PerfectOperationsAndSwapOut'].map((n) => [n, art(n)]));
export const NATIVE = '0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE';
const MULTISIG = '0x4F6F977aCDD1177DCD81aB83074855EcB9C2D49e';
export const DEAD = '0x000000000000000000000000000000000000dEaD';
export const POOL = new Interface([...A.Dex_USDC_ETH.abi.filter((f) => f.type !== 'constructor'),
  ...A.DexT1Admin.abi.filter((f) => f.type === 'function' && f.name === 'initialize')]);
const LIQ = new Interface([...A.Liquidity.abi.filter((f) => f.type !== 'constructor'),
  ...A.AdminModule.abi.filter((f) => f.type === 'function'), ...A.UserModule.abi.filter((f) => f.type === 'function')]);
const CV = A.Dex_USDC_ETH.abi.find((f) => f.type === 'constructor').inputs;
const T0 = 1_767_200_000n; // время блока окружения (Liquidity считает проценты по времени; 0 — не время сети)

/// Слоты слоя ликвидности (LiquiditySlotsLink: supply 8, borrow 9 — двойные отображения; exchangePrices 5).
const map = (key, slot) => keccak256(coder.encode(['address', 'uint256'], [key, slot]));
const dmap = (slot, a, b) => keccak256(coder.encode(['address', 'bytes32'], [b, map(a, slot)]));
export function slots(dex, t0, t1) {
  return [dmap(8n, dex, t0), dmap(9n, dex, t0), dmap(8n, dex, t1), dmap(9n, dex, t1), map(t0, 5n), map(t1, 5n)];
}

const call = async (c, to, iface, fn, args, value = 0n) => {
  const r = await L.call(c, LP, to, iface.encodeFunctionData(fn, args), value);
  if (!r.ok) throw new Error(`${fn}: откат ${r.reason} ${r.ret.slice(0, 74)}`);
  return r;
};

/// Пул токенов t0 < t1 (NATIVE — метка нативной монеты): операции, пул, лимиты в слое ликвидности, initialize с
/// ликвидностью lq токена 0 (цена 1:1, оба токена 18 знаков). Возвращает адрес пула.
async function poolOf(c, f, t0, t1, id, lq) {
  const nonce = (await c.sm.getAccount(L.addr(LP))).nonce;
  const ops = async (n) => L.deploy(c, LP, A[n], [CV[0]], [cv(0n)]);
  const dex = getCreateAddress({ from: LP, nonce: nonce + 3n }); // после трёх операций
  const z = '0x' + '00'.repeat(20);
  const cv = (fill, impl = [f.shift, z, z, z, z]) => [id, f.liq, f.fac, impl, LP, t0, t1, ...slots(dex, t0, t1), 49152n];
  const col = await ops('Dex_USDC_ETH_ColOperations');
  const debt = await ops('Dex_USDC_ETH_DebtOperations');
  const perf = await ops('Dex_USDC_ETH_PerfectOperationsAndSwapOut');
  const pool = await L.deploy(c, LP, A.Dex_USDC_ETH, [CV[0]], [cv(1n, [f.shift, f.admin, col, debt, perf])]);
  if (pool.toLowerCase() !== dex.toLowerCase()) throw new Error(`адрес пула ${pool} ≠ ${dex}`);
  for (const t of [t0, t1]) {
    await call(c, f.liq, LIQ, 'updateUserSupplyConfigs', [[[pool, t, 1, 2000n, 2n * 86400n, 100_000n * E18 * 1000n]]]);
    await call(c, f.liq, LIQ, 'updateUserBorrowConfigs', [[[pool, t, 1, 2000n, 2n * 86400n, 100_000n * E18, 1_000_000n * E18]]]);
    if (t !== NATIVE) await must(c, LP, t, ERC20.abi, 'approve', [pool, 10n ** 30n]);
  }
  const init = [true, lq, false, 0n, 10n ** 27n, 100n, 0n, 100_000n, 100_000n, 50_000n, 50_000n, 86400n, 0n, 0n,
    (10n ** 27n * 120n) / 100n, (10n ** 27n * 80n) / 100n];
  await call(c, pool, POOL, 'initialize', [init], t1 === NATIVE ? lq : 0n);
  return pool;
}

/// Мир: world.mjs (исполнитель art) + слой ликвидности, фабрика, пулы USDT/COIN и (COIN, ETH), разрешения владельца.
export async function env(art0 = v2) {
  const w = await world(art0);
  const { c, usdt, coin, ex } = w;
  c.ts = T0;
  // initialize пула проверяет, что TEAM_MULTISIG — контракт (adminModule/main.sol L654-L655; адрес — константа кода,
  // common/constantVariables.sol L15; в сети это мультисиг, он же admin Liquidity в аргументах артефакта). Здесь —
  // код из одного STOP на этом адресе.
  await c.sm.putContractCode(L.addr(MULTISIG), new Uint8Array([0]));
  const dummy = await L.deploy(c, LP, A.LiquidityDummyImpl);
  const liq = await L.deploy(c, LP, A.Liquidity, ['address', 'address'], [LP, dummy]);
  const adm = await L.deploy(c, LP, A.AdminModule, ['uint256'], [BigInt(A.AdminModule.args[0])]);
  const usr = await L.deploy(c, LP, A.UserModule);
  const sigs = (a) => new Interface(a.abi).fragments.filter((x) => x.type === 'function').map((x) => x.selector);
  await call(c, liq, LIQ, 'addImplementation', [adm, sigs(A.AdminModule)]);
  await call(c, liq, LIQ, 'addImplementation', [usr, sigs(A.UserModule)]);
  await call(c, liq, LIQ, 'updateAuths', [[[LP, true]]]);
  const toks = [usdt, coin, NATIVE];
  await call(c, liq, LIQ, 'updateRateDataV1s', [toks.map((t) => [t, 8000n, 400n, 1000n, 15000n])]);
  await call(c, liq, LIQ, 'updateTokenConfigs', [toks.map((t) => [t, 0n, 0n, 10000n])]);
  const fac = await L.deploy(c, LP, A.DexFactory, ['address'], [LP]);
  await call(c, fac, new Interface(A.DexFactory.abi), 'setGlobalAuth', [LP, true]);
  const admin = await L.deploy(c, LP, A.DexT1Admin);
  const shift = await L.deploy(c, LP, A.DexT1Shift, ['address'], [LP]);
  const f = { liq, fac, admin, shift };
  const [a, b] = BigInt(usdt) < BigInt(coin) ? [usdt, coin] : [coin, usdt];
  const pU = await poolOf(c, f, a, b, 1n, 10_000n * E18);
  if (BigInt(coin) >= BigInt(NATIVE)) throw new Error('COIN ≥ метки нативной монеты — порядок токенов пула иной');
  const pN = await poolOf(c, f, coin, NATIVE, 2n, 1_000n * E18);
  for (const t of [usdt, coin]) await must(c, OWNER, t, ERC20.abi, 'approve', [ex, 10n ** 30n]);
  await must(c, LP, coin, ERC20.abi, 'transfer', [OWNER, 10_000n * E18]);
  return { ...w, ...f, pU, pN, t0U: a, t1U: b };
}
