import numpy as np, pandas as pd
IS_END='2020-12-31'; OOS=('2021-01-01','2024-12-31'); HO_START='2025-01-01'
def periods(start):
    return {'IS':(start,IS_END),'OOS':OOS,'HOLDOUT':(HO_START,'2099-12-31')}
def blend(px):
    lp=np.log(px); s=sum(np.sign(lp-lp.shift(n)) for n in (63,126,252))/3
    return s.where(lp.shift(252).notna())
def sma(px):
    s=np.sign(px.rolling(50).mean()-px.rolling(200).mean()); return s.where(px.shift(200).notna())
def vol(px,ann): return np.log(px).diff().rolling(60).std()*np.sqrt(ann)
def run_weights(rets, target_w, cost_side, rebal_every=5):
    """target_w decided at close t; executed at close t+1; earns from t+2 close-to-close returns."""
    tw=target_w.copy()
    mask=np.zeros(len(tw),bool); mask[::rebal_every]=True
    tw=tw.where(pd.Series(mask,index=tw.index),np.nan).ffill().fillna(0)
    held=tw.shift(1).fillna(0)          # executed at close t+1 -> held from t+1
    gross=(held.shift(1).fillna(0)*rets.fillna(0)).sum(axis=1)
    dw=held.diff().abs(); dw.iloc[0]=held.iloc[0].abs()
    cost=(dw*cost_side).sum(axis=1) if np.ndim(cost_side)==0 else (dw*cost_side.reindex_like(dw).fillna(0)).sum(axis=1)
    sgn=np.sign(held); trades=((sgn.diff().fillna(sgn)!=0)).sum(axis=1)
    return pd.DataFrame({'gross':gross,'cost':cost,'net':gross-cost,'turn':dw.sum(axis=1),'trades':trades})
def maxdd(eq): return float((eq/eq.cummax()-1).min())
def sharpe(r,ann): return float(r.mean()/r.std()*np.sqrt(ann)) if r.std()>0 else np.nan
def boot5(r,ann,block=20,n=1000,seed=7):
    r=r.values; L=len(r)
    if L<block*3: return np.nan
    rng=np.random.default_rng(seed); out=[]
    nb=int(np.ceil(L/block))
    for _ in range(n):
        st=rng.integers(0,L,nb); idx=(st[:,None]+np.arange(block)[None,:])%L
        x=r[idx.ravel()[:L]]; out.append(x.mean()/x.std()*np.sqrt(ann) if x.std()>0 else 0)
    return float(np.percentile(out,5))
def metrics(df,ann,bench=None,block=20):
    r=df['net']
    if len(r)<2: return {}
    eq=(1+r).cumprod(); yrs=len(r)/ann
    cagr=eq.iloc[-1]**(1/yrs)-1
    m={'start':str(r.index[0].date()),'end':str(r.index[-1].date()),'CAGR':cagr,'Sharpe':sharpe(r,ann),'MaxDD':maxdd(eq),
       'trades':int(df['trades'].sum()),'turnover_pa':float(df['turn'].sum()/yrs),'cost_pa':float(df['cost'].sum()/yrs),
       'gross_CAGR':float((1+df['gross']).prod()**(1/yrs)-1)}
    yr=(1+r).groupby(r.index.year).prod()-1; m['pos_years']=float((yr>0).mean())
    m['Calmar']=cagr/abs(m['MaxDD']) if m['MaxDD']<0 else np.nan
    m['boot5_Sharpe']=boot5(r,ann,block)
    if bench is not None:
        b=bench.reindex(r.index).fillna(0); be=(1+b).prod()**(1/yrs)-1
        m['bench_CAGR']=be; m['edge']=cagr-be
    else: m['edge']=cagr
    return m
def score(m,trades_total):
    def band(x,cuts):  # cuts ascending, return 1..5
        return 1+sum(x>=c for c in cuts)
    s={'Sharpe':band(m['Sharpe'],[0,0.3,0.6,1.0]) if m['Sharpe']>=0 else 1,
       'Calmar':(band(m['Calmar'],[0,0.2,0.5,1.0]) if m['Calmar']>=0 else 1) if m['Calmar']==m['Calmar'] else 1,
       'MaxDD':band(-abs(m['MaxDD']),[-0.5,-0.35,-0.25,-0.15]),
       'Trades':band(m['trades'],[30,60,120,250]),
       'Edge':band(m['edge'],[0,0.02,0.05,0.10]) if m['edge']>=0 else 1}
    # fix lower bands: values below first cut get 1
    s['Sharpe']=1 if m['Sharpe']<0 else min(5,band(m['Sharpe'],[0.3,0.6,1.0])+1)
    s['Calmar']=1 if not (m['Calmar']>=0) else min(5,band(m['Calmar'],[0.2,0.5,1.0])+1)
    s['MaxDD']=band(-abs(m['MaxDD']),[-0.5,-0.35,-0.25,-0.15]) if abs(m['MaxDD'])<=0.5 else 1
    s['Trades']=1 if m['trades']<30 else min(5,band(m['trades'],[60,120,250])+1)
    s['Edge']=1 if m['edge']<0 else min(5,band(m['edge'],[0.02,0.05,0.10])+1)
    s['total']=sum(s.values()); return s
def window(df,a,b): return df.loc[a:b]
