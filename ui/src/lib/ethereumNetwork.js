// Каноническая сеть для источников котировок строителей Ethereum.
export function ethereumNetwork(venue,profiles,values) {
  const chain=values['dex.chain|Venue|'+venue], id=values['dex.evm_chain_id|Venue|'+venue];
  const profile=profiles?.find(p=>p.venue===venue) ?? profiles?.find(p=>p.evm_chain_id===id);
  return chain==='ethereum' && id===1 && profile?.chain===chain && profile.evm_chain_id===id ? id : null;
}
