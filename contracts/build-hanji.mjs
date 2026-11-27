// Builds a standalone execution contract with the compiler in contracts/package-lock.json.
// The original ArbExecutor and its versioned artifacts use their existing independent build.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import solc from 'solc';

const output = process.argv[2];
if (!output) throw new Error('Expected an artifact directory');
const name = process.argv[3] ?? 'HanjiExecutor';
if (!['HanjiExecutor', 'FermiSlotExecutor', 'SolidlyV3Executor', 'ElfomoExecutor', 'BasePmmExecutor', 'FraxswapAdapter', 'OasisSwapAdapter', 'EtherVistaAdapter','MemeboxAdapter','BunnySwapAdapter','CandySwapAdapter','MintClubAdapter','CarbonWallet','RubiconClassicWallet','StabullAdapter','UniswapV1Adapter','MooniswapWallet','BancorV3Adapter','BancorLegacyAdapter','AktionariatWallet','BalancerV1Adapter','YieldBasisAdapter','AntfarmWallet','AntfarmOutputWallet','TempleAdapter','SurgeAdapter','SafeSwapAdapter','DegenSwapAdapter','BundlesSwapAdapter','O1NativeAdapter','BaseNativeMarkets','UmbraeAdapter','YumyumAdapter','BluedexAdapter','VvsV3Adapter','CommunityV3Adapter','ShibaswapV2Adapter','AlgebraRouterWallet','BopAmmWallet','DagrowAdapter','PendleMarketAdapter','MaverickV1Adapter','NativeRfqAdapter','GladiusInventory','LlammaWallet','TextileWallet','DefiPlazaWallet','RaindexWallet','RaindexCashWallet','AngstromWallet','AmmalgamWallet','AquaStrategyWallet','EmpireWallet','EpsilonWallet','EpsilonApiWallet','IntegralSizeAdapter','PropVenueAdapter'].includes(name)) throw new Error('Unknown executor');
const file = name + '.sol';
const root = import.meta.dirname;
const files = name === 'BaseNativeMarkets' ? [file, 'BaseNativeSettlement.sol', 'IPools.sol'] : name === 'BasePmmExecutor' ? [file, 'IBasePmm.sol', 'BaseNativeV3.sol', 'BaseVirtuals.sol'] : ['YumyumAdapter','BluedexAdapter','VvsV3Adapter','CommunityV3Adapter','ShibaswapV2Adapter','DagrowAdapter','PendleMarketAdapter'].includes(name) ? [file, 'RegisteredV3Adapter.sol', 'IPools.sol'] : ['StabullAdapter','UniswapV1Adapter','MooniswapWallet','BancorV3Adapter','BancorLegacyAdapter','AktionariatWallet','BalancerV1Adapter','YieldBasisAdapter','PropVenueAdapter','MaverickV1Adapter','UmbraeAdapter','O1NativeAdapter','BundlesSwapAdapter'].includes(name) ? [file,'IPools.sol'] : name==='DegenSwapAdapter' ? [file,'SafeSwapAdapter.sol'] : name==='AntfarmOutputWallet' ? [file,'AntfarmWallet.sol'] : [file];
const input = {
  language: 'Solidity',
  sources: Object.fromEntries(files.map((path) => [path, { content: readFileSync(join(root, path), 'utf8').replace(/^\uFEFF/, '') }])),
  settings: {
    optimizer: { enabled: true, runs: 200 }, viaIR: true, evmVersion: 'cancun',
    outputSelection: { '*': { '*': ['abi', 'evm.bytecode.object', 'evm.deployedBytecode.object', 'evm.deployedBytecode.immutableReferences', 'storageLayout'] } },
  },
};
const result = JSON.parse(solc.compile(JSON.stringify(input)));
for (const error of (result.errors ?? [])) {
  if (error.severity === 'error') throw new Error(error.formattedMessage);
}
const contract = result.contracts[file][name];
if (!contract.evm.deployedBytecode.object || contract.evm.deployedBytecode.object.length / 2 > 24576)
  throw new Error(name + ' exceeds the EVM contract size limit or is empty');
mkdirSync(output, { recursive: true });
writeFileSync(join(output, name + '.json'), JSON.stringify({
  solc: solc.version(), abi: contract.abi, creation: contract.evm.bytecode.object,
  runtime: contract.evm.deployedBytecode.object, storage: contract.storageLayout.storage,
  ...(['FraxswapAdapter','OasisSwapAdapter','EtherVistaAdapter','MemeboxAdapter','BunnySwapAdapter','CandySwapAdapter','MintClubAdapter','CarbonWallet','RubiconClassicWallet','StabullAdapter','UniswapV1Adapter','MooniswapWallet','BancorV3Adapter','BancorLegacyAdapter','AktionariatWallet','BalancerV1Adapter','YieldBasisAdapter','AntfarmWallet','AntfarmOutputWallet','TempleAdapter','SurgeAdapter','SafeSwapAdapter','DegenSwapAdapter','BundlesSwapAdapter','O1NativeAdapter','BaseNativeMarkets','UmbraeAdapter','YumyumAdapter','BluedexAdapter','VvsV3Adapter','CommunityV3Adapter','ShibaswapV2Adapter','AlgebraRouterWallet','BopAmmWallet','DagrowAdapter','PendleMarketAdapter','MaverickV1Adapter','NativeRfqAdapter','GladiusInventory','LlammaWallet','TextileWallet','DefiPlazaWallet','RaindexWallet','RaindexCashWallet','AngstromWallet','AmmalgamWallet','AquaStrategyWallet','EmpireWallet','EpsilonWallet','EpsilonApiWallet','IntegralSizeAdapter','PropVenueAdapter'].includes(name) ? { immutableReferences: contract.evm.deployedBytecode.immutableReferences } : {}),
}));
