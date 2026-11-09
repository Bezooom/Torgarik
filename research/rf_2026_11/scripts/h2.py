import pandas as pd, numpy as np, json, lib
s=pd.read_csv('data/tqbr_monthend.csv')
s['px']=s.CLOSE.fillna(s.LEGALCLOSEPRICE)
s=s[(s.px>0)]
s['pri']=(s.board=='TQBR').astype(int)
s=s.sort_values('pri').drop_duplicates(['me','SECID'],keep='last')
P=s.pivot(index='me',columns='SECID',values='px'); V=s.pivot(index='me',columns='SECID',values='VALUE').fillna(0)
P.index=pd.to_datetime(P.index); V.index=pd.to_datetime(V.index)
# corporate-action detection on monthly ratio
ratio=P/P.ffill().shift(1)
ca=(ratio>4)|(ratio<0.25)
events=[(str(d.date()),c,round(float(ratio.at[d,c]),4)) for d,c in zip(*np.where(ca)) for d,c in [(P.index[d],P.columns[c])]]
print('CA events',events)
R=ratio.where(~ca)-1           # monthly returns; split months excluded (set NaN)
# adjusted price for momentum: chain returns
A=(1+R.fillna(0)).cumprod().where(P.notna())
liq=V.shift(1).rolling(3,min_periods=3).mean()
mom=A.ffill().shift(1)/A.ffill().shift(12)-1
mom=mom.where(P.shift(1).notna()&P.shift(12).notna()&P.notna())
dates=P.index; W=pd.DataFrame(0.0,index=dates,columns=P.columns); univ_sizes=[]
for d in dates:
    m=mom.loc[d].dropna(); l=liq.loc[d].reindex(m.index).dropna()
    l=l[l>0]
    if len(l)<20: continue
    u=l.sort_values(ascending=False).index[:35]; univ_sizes.append(len(u))
    top=m[u].sort_values(ascending=False).index[:len(u)//3]
    W.loc[d,top]=1/len(top)
first=W.index[W.sum(axis=1)>0][0]
# next-month return: if missing next month-end price, hold at 0 return (noted)
NR=R.shift(-1)
miss=((W>0)&NR.isna()).sum().sum()
gross=(W*NR.fillna(0)).sum(axis=1).shift(1)   # realised in month d+1, indexed at d+1
dw=W.diff().abs(); dw.iloc[0]=W.iloc[0]
cost=(dw.sum(axis=1)*0.001)
trades=((W>0).astype(int).diff().abs()).sum(axis=1)
out=pd.DataFrame({'gross':gross.fillna(0),'cost':cost.shift(1).fillna(0),'turn':dw.sum(axis=1).shift(1).fillna(0),'trades':trades.shift(1).fillna(0)})
out['net']=out.gross-out.cost
out=out.loc[first+pd.offsets.MonthEnd(1):]
out=out.iloc[1:] if out.index[0]<=first else out
ix=pd.read_csv('data/IMOEX.csv',parse_dates=['TRADEDATE']).set_index('TRADEDATE').CLOSE
tr=pd.read_csv('data/MCFTR.csv',parse_dates=['TRADEDATE']).set_index('TRADEDATE').CLOSE
bi=ix.reindex(P.index).pct_change(); bt=tr.reindex(P.index).pct_change()
# equal-weight universe benchmark (all top-35 liquid, info)
out.to_csv('out_H2_mom.csv'); pd.DataFrame({'IMOEX':bi,'MCFTR':bt}).to_csv('out_H2_bench.csv')
res={'events':events,'missing_next_price_positions':int(miss),'first_portfolio':str(first.date()),'avg_universe':float(np.mean(univ_sizes))}
P_=lib.periods(str(out.index[0].date()))
for k,(a,b) in {**P_,'covid':('2020-02-01','2020-04-30'),'y2022':('2022-01-01','2022-12-31'),'FULL':('2000','2099')}.items():
    m=lib.metrics(out.loc[a:b],12,bench=bi,block=3); res[k]=m
    bb=pd.DataFrame({'net':bi.reindex(out.loc[a:b].index).fillna(0),'gross':0,'cost':0,'turn':0,'trades':0})
    mb=lib.metrics(bb,12,block=3); res[k+'_IMOEX']=mb
    print(f"{k:8s} {m['start']}..{m['end']} CAGR {m['CAGR']:+.1%} IMOEX {m['bench_CAGR']:+.1%} edge {m['edge']:+.1%} Sh {m['Sharpe']:.2f} (IMOEX {mb['Sharpe']:.2f}) DD {m['MaxDD']:.1%} (IMOEX {mb['MaxDD']:.1%}) tr {m['trades']} turn {m['turnover_pa']:.1f} b5 {m['boot5_Sharpe']:.2f} pos {m['pos_years']:.0%}")
print('missing',miss,'universe',np.mean(univ_sizes),'first',first)
json.dump(res,open('res_h2.json','w'),indent=1,default=float)
