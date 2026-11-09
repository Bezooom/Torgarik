import pandas as pd, numpy as np, json, glob, os, lib
lib.IS_END='2022-12-31'; lib.OOS=('2023-01-01','2024-12-31')
P=pd.read_pickle('data/P2.pkl').loc[:'2026-11-08']; Q=pd.read_pickle('data/Q2.pkl').loc[:'2026-11-08']
F={}
for f in glob.glob('data/fund2/*.csv'):
    d=pd.read_csv(f)
    if len(d): d['date']=pd.to_datetime(d.time,format='mixed').dt.floor('D'); F[os.path.basename(f)[:-4]]=d.groupby('date').fundingRate.sum()
F=pd.DataFrame(F).reindex(P.index)
print('funding symbols',F.shape[1],'of',P.shape[1])
R=P.pct_change(fill_method=None)
age=P.notna().cumsum(); lv=Q.rolling(30,min_periods=30).mean()
ok=(age>=100)&(Q.rolling(5).min()>0)
U=(lv.where(ok).rank(axis=1,ascending=False)<=25)
sig=lib.vol(P,365)
def xs(score,longshort=True,every=7,nq=5):
    sc=score.where(U&sig.notna()&score.notna())
    rk=sc.rank(axis=1,ascending=False)   # 1 = highest score
    n=sc.notna().sum(axis=1)
    top=(rk<=nq)&(n>=20).values[:,None]; bot=(rk>n.values[:,None]-nq)&(n>=20).values[:,None]
    if longshort:
        w=(top.astype(float)-bot.astype(float))*(0.20/np.sqrt(2*nq))/sig
    else:
        w=top.astype(float)*(0.20/np.sqrt(nq))/sig
    w=w.clip(-0.25,0.25).fillna(0)
    if not longshort:
        s=w.sum(axis=1); w=w.div(np.maximum(s,1),axis=0)
    return w
def run(w,every):
    rets=R.fillna(0)-F.fillna(0)      # long pays funding, short (w<0) receives: w*(r-f)
    return lib.run_weights(rets[w.columns],w,0.001,rebal_every=every)
out={}
# C1 momentum
lr=np.log(P.ffill())
score=sum((lr-lr.shift(n)).where(U).rank(axis=1) for n in (7,30,90))/3
out['C1_mom_ls']=run(xs(score,True),7); out['C1_mom_long']=run(xs(score,False),7)
# C2 funding carry: short highest funding -> score=funding, long/short flipped
fs=F.rolling(3,min_periods=3).mean()
w2=-xs(fs,True); out['C2_carry_ls']=run(w2,3)
# C3 trend + funding filter on H3 spot universe
coins=['BTC','ETH','BNB','XRP','LTC','ADA','TRX','EOS','XLM','ETC']
SP=pd.concat({c:pd.read_csv(f'data/spot_{c}.csv',parse_dates=['date']).set_index('date').close for c in coins},axis=1).sort_index().loc[:'2026-11-08']
SR=SP.pct_change(fill_method=None)
FC=pd.concat({c:F[c+'USDT'] for c in coins},axis=1).reindex(SP.index)
f7=FC.rolling(7,min_periods=7).mean()
pct=f7.rolling(365,min_periods=180).apply(lambda x:(x[:-1]<x[-1]).mean() if len(x)>1 else np.nan,raw=True)
s=lib.blend(SP); v=lib.vol(SP,365); act=s.notna()&v.notna()&SP.notna(); n=act.sum(axis=1).replace(0,np.nan)
base=(0.20/np.sqrt(n)).values[:,None]/v; cap=(1/n).values[:,None]
wl=(s.clip(lower=0)*base).where(act).clip(upper=cap).fillna(0)
crowd=(pct>=0.90)
wC3=wl.where(~crowd,wl*0.5)
wsh=((s<0)&crowd&act).astype(float)*(-base).clip(lower=-cap)
wC3s=wC3+wsh.fillna(0)
for k,w in [('H3_plain',wl),('C3_filter',wC3),('C3_filter_short',wC3s)]:
    w=w.where(SP.notna(),0)
    rets=SR.fillna(0)-(FC.fillna(0)*(w<0))  # short legs via perp: w*(r-f) for shorts
    rets=SR.fillna(0)
    o=lib.run_weights(rets,w,0.001)
    if k=='C3_filter_short':  # funding on short legs: short (w<0) receives f -> pnl += -w*f... = |w|*f
        held=w.copy(); m=np.zeros(len(held),bool); m[::5]=True
        held=held.where(pd.Series(m,index=held.index),np.nan).ffill().fillna(0).shift(1).fillna(0)
        fp=(-(held.shift(1).fillna(0).clip(upper=0))*FC.fillna(0)).sum(axis=1)
        o['gross']+=fp; o['net']+=fp
    out[k]=o
res={}
for k,o in out.items():
    first=o.index[o.turn>0][0]; o=o.loc[first:]; o.to_csv(f'out2_{k}.csv')
    Pp=lib.periods(str(first.date()))
    res[k]={p:lib.metrics(o.loc[a:b],365) for p,(a,b) in Pp.items()}
    res[k]['FULL']=lib.metrics(o,365)
    yr=((1+o.net).groupby(o.index.year).prod()-1); res[k]['years']={int(y):float(x) for y,x in yr.items()}
    sc=lib.score(res[k]['OOS'],0); o_,h_=res[k]['OOS'],res[k]['HOLDOUT']
    res[k]['score']=sc; res[k]['pass']=bool(sc['total']>=18 and o_['trades']+h_['trades']>=30 and o_['edge']>0 and h_['edge']>0)
    print(k,'start',first.date(),sc,'PASS' if res[k]['pass'] else 'FAIL')
    for p in ['IS','OOS','HOLDOUT']:
        m=res[k][p]; print(f"   {p:8s} {m['start']}..{m['end']} CAGR {m['CAGR']:+.1%} Sh {m['Sharpe']:.2f} DD {m['MaxDD']:.1%} tr {m['trades']} turn {m['turnover_pa']:.0f} cost {m['cost_pa']:.1%} gross {m['gross_CAGR']:+.1%} b5 {m['boot5_Sharpe']:.2f} pos {m['pos_years']:.0%}")
    print('   years',' '.join(f"{y}:{x:+.0%}" for y,x in res[k]['years'].items()))
json.dump(res,open('res_round2.json','w'),indent=1,default=float)
