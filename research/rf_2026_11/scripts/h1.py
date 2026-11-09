import pandas as pd, numpy as np, json, lib
df=pd.read_csv('data/fut_contracts.csv',parse_dates=['date'])
df=df[df.settle>0]
TICK={'Si':1,'RI':10,'MX':25,'GD':0.1,'BR':0.01,'NG':0.001}
rets={};costside={};rollc={};info={}
for a,g in df.groupby('asset'):
    S=g.pivot_table(index='date',columns='contract',values='settle',aggfunc='last').sort_index()
    V=g.pivot_table(index='date',columns='contract',values='volume',aggfunc='last').reindex(S.index).fillna(0)
    order=g.drop_duplicates('contract').sort_values(['year','mon']).contract.tolist()
    last={c:S[c].last_valid_index() for c in order}
    order=[c for c in order if last[c] is not None]
    dates=S.index; held=None; r=pd.Series(0.0,index=dates); roll=pd.Series(0.0,index=dates); hc=[]
    pos={d:i for i,d in enumerate(dates)}
    for i,t in enumerate(dates):
        if held is not None and i>0:
            p0,p1=S.at[dates[i-1],held],S.at[t,held]
            r.iloc[i]=p1/p0-1 if (p0==p0 and p1==p1) else 0.0
        # choose/roll at close t
        avail=[c for c in order if S.at[t,c]==S.at[t,c]]
        if held is None:
            # nearest contract with volume >0
            cand=[c for c in avail if V.at[t,c]>0]
            if cand: held=cand[0]
        else:
            idx=order.index(held); nxt=[c for c in order[idx+1:] if c in avail]
            if nxt:
                n=nxt[0]; days_left=pos.get(last[held],len(dates))-i
                if V.at[t,n]>V.at[t,held] or days_left<=5 or S.at[t,held]!=S.at[t,held]:
                    held=n; roll.iloc[i]=1
        hc.append(held)
    px=(1+r).cumprod(); first=next(i for i,h in enumerate(hc) if h)
    rets[a]=r.iloc[first+1:]; rollc[a]=roll
    held_px=pd.Series([S.at[d,h] if h else np.nan for d,h in zip(dates,hc)],index=dates)
    costside[a]=0.0002+TICK[a]/held_px
    info[a]={'start':str(dates[first].date()),'end':str(dates[-1].date()),'contracts':len(set(hc)-{None}),'rolls':int(roll.sum()),
             'max_abs_daily':float(r.abs().max()),'maxday':str(r.abs().idxmax().date())}
R=pd.DataFrame(rets).sort_index(); R=R.loc[:'2026-11-06']
print(json.dumps(info,indent=0))
# big moves check
print((R.abs()>0.15).sum())
for a in R: 
    big=R[a][R[a].abs()>0.15]; print(a, [(str(d.date()),round(x,3)) for d,x in big.items()][:10])
PX=(1+R.fillna(0)).cumprod().where(R.notna())
CS=pd.DataFrame(costside).reindex(R.index).ffill(); RL=pd.DataFrame(rollc).reindex(R.index).fillna(0)
def h1(sigfun):
    s=sigfun(PX); v=lib.vol(PX,252); act=s.notna()&v.notna()&R.notna()
    n=act.sum(axis=1).replace(0,np.nan)
    w=(s*(0.15/np.sqrt(n)).values[:,None]/v).where(act).clip(-3,3).fillna(0)
    out=lib.run_weights(R,w,CS)
    # roll cost: held weight at roll day * 2 legs
    tw=w.copy(); mask=np.zeros(len(tw),bool); mask[::5]=True
    held=tw.where(pd.Series(mask,index=tw.index),np.nan).ffill().fillna(0).shift(1).fillna(0)
    rc=(held.abs()*RL*2*CS).sum(axis=1)
    out['cost']+=rc; out['net']-=rc
    return out
res={}
for k,f in [('H1_blend',lib.blend),('H1_sma',lib.sma)]:
    o=h1(f); first=o.index[o.turn>0][0]; o=o.loc[first:]; o.to_csv(f'out_{k}.csv')
    P=lib.periods(str(first.date()))
    res[k]={p:lib.metrics(o.loc[a:b],252) for p,(a,b) in P.items()}
    res[k]['covid']=lib.metrics(o.loc['2020-02-15':'2020-04-30'],252); res[k]['y2022']=lib.metrics(o.loc['2022'],252); res[k]['FULL']=lib.metrics(o,252)
    for p in ['IS','OOS','HOLDOUT','covid','y2022']:
        m=res[k][p]; print(k,f"{p:8s} {m['start']}..{m['end']} CAGR {m['CAGR']:+.1%} Sh {m['Sharpe']:.2f} DD {m['MaxDD']:.1%} tr {m['trades']} turn {m['turnover_pa']:.1f} cost {m['cost_pa']:.2%} b5 {m['boot5_Sharpe']:.2f} pos {m['pos_years']:.0%}")
# per-asset stand-alone OOS sharpe (info)
pa={}
for a in R:
    s=lib.blend(PX[[a]]); v=lib.vol(PX[[a]],252); w=(s*0.15/v).clip(-3,3).fillna(0)
    o=lib.run_weights(R[[a]],w,CS[[a]])
    pa[a]={p:round(lib.sharpe(o.net.loc[x:y],252),2) for p,(x,y) in lib.periods('2000').items()}
print(pa); res['per_asset']=pa; res['info']=info
json.dump(res,open('res_h1.json','w'),indent=1,default=float)
