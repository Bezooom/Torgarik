import pandas as pd, numpy as np, json, lib
coins=['BTC','ETH','BNB','XRP','LTC','ADA','TRX','EOS','XLM','ETC']
px=pd.concat({c:pd.read_csv(f'data/spot_{c}.csv',parse_dates=['date']).set_index('date').close for c in coins},axis=1).sort_index()
px=px.loc[:'2026-11-08']
rets=px.pct_change()
res={}
def h3(sigfun,longonly):
    s=sigfun(px); 
    if longonly: s=s.clip(lower=0)
    v=lib.vol(px,365); act=s.notna()&v.notna()&px.notna()
    n=act.sum(axis=1).replace(0,np.nan)
    w=(s*(0.20/np.sqrt(n)).values[:,None]/v).where(act)
    cap=(1/n).values[:,None]
    w=w.clip(lower=-cap,upper=cap).fillna(0)
    # asset delisted -> after last price weight 0
    w=w.where(px.ffill().notna()&px.notna(),0)
    return lib.run_weights(rets,w,0.001)
start='2019-01-01'
out={'H3_blend_longflat':h3(lib.blend,True),'H3_blend_longshort':h3(lib.blend,False),'H3_sma_longflat':h3(lib.sma,True)}
# BTC buy&hold and equal-weight B&H benchmark (info)
bh=pd.DataFrame({'net':rets.BTC.fillna(0),'gross':rets.BTC.fillna(0),'cost':0,'turn':0,'trades':0})
out['BTC_buyhold']=bh
# H4
cc=['BTC','ETH','BNB','XRP','LTC','ADA']
sp=pd.concat({c:pd.read_csv(f'data/spot_{c}.csv',parse_dates=['date']).set_index('date').close for c in cc},axis=1)
pp=pd.concat({c:pd.read_csv(f'data/perp_{c}.csv',parse_dates=['date']).set_index('date').close for c in cc},axis=1)
fd={}
for c in cc:
    f=pd.read_csv(f'data/funding_{c}.csv',parse_dates=['time']); f['date']=f.time.dt.floor('D')
    fd[c]=f.groupby('date').fundingRate.sum()
fd=pd.concat(fd,axis=1)
idx=pp.index.intersection(sp.index); idx=idx[idx<= '2026-11-08']
sp,pp,fd=sp.reindex(idx),pp.reindex(idx),fd.reindex(idx)
ann_f=fd.rolling(7).mean()*365
sig=(ann_f>0.10).astype(float).where(ann_f.notna()&pp.notna()&sp.notna())
n=sig.notna().sum(axis=1).replace(0,np.nan)
w=(sig/n.values[:,None]).fillna(0)
# carry return per unit notional: spot ret - perp ret + funding (funding on day t paid on positions held through day t)
carry=(sp.pct_change()-pp.pct_change()+fd).fillna(0)
cost_side=0.0010+0.0005+2*0.0002   # both legs per enter/exit
out['H4_funding_carry']=lib.run_weights(carry,w,cost_side,rebal_every=1)
out['H4_funding_only_nobasis']=lib.run_weights(fd.fillna(0),w,cost_side,rebal_every=1)
for k,df in out.items():
    first=df.index[(df.turn>0)|(df.net!=0)][0] if k!='BTC_buyhold' else pd.Timestamp(start)
    df=df.loc[max(first,pd.Timestamp('2019-01-01') if k.startswith('H3') or k.startswith('BTC') else first):]
    out[k]=df
    P=lib.periods(str(df.index[0].date()))
    res[k]={p:lib.metrics(df.loc[a:b],365) for p,(a,b) in P.items()}
    res[k]['covid']=lib.metrics(df.loc['2020-02-15':'2020-04-30'],365)
    res[k]['y2022']=lib.metrics(df.loc['2022-01-01':'2022-12-31'],365)
    res[k]['FULL']=lib.metrics(df,365)
    df.to_csv(f'out_{k}.csv')
json.dump(res,open('res_crypto.json','w'),indent=1,default=float)
for k,v in res.items():
    print(k)
    for p in ['IS','OOS','HOLDOUT','covid','y2022']:
        m=v[p]; print(f"  {p:8s} {m['start']}..{m['end']} CAGR {m['CAGR']:+.1%} Sh {m['Sharpe']:.2f} DD {m['MaxDD']:.1%} tr {m['trades']} turn {m['turnover_pa']:.1f} cost {m['cost_pa']:.2%} b5 {m['boot5_Sharpe']:.2f} pos {m['pos_years']:.0%}")
