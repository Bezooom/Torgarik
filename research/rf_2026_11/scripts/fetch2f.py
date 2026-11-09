import subprocess, json, os, pandas as pd, numpy as np, concurrent.futures as cf, time, glob
def curl(u,t=30):
    for a in range(4):
        p=subprocess.run(['curl','-s','-m',str(t),'--connect-timeout','8',u],capture_output=True,text=True)
        if p.returncode==0 and p.stdout: return p.stdout
        time.sleep(2)
    return None
P={};Q={}
for f in glob.glob('data/perp2/*.csv'):
    s=os.path.basename(f)[:-4]; d=pd.read_csv(f,parse_dates=['date'])
    if len(d): P[s]=d.set_index('date').close; Q[s]=d.set_index('date').qvol
P=pd.DataFrame(P).sort_index(); Q=pd.DataFrame(Q).reindex(P.index)
P.to_pickle('data/P2.pkl'); Q.to_pickle('data/Q2.pkl')
age=P.notna().cumsum(); lv=Q.rolling(30,min_periods=30).mean()
ok=(age>=100)&(Q.rolling(5).min()>0)
U=lv.where(ok).rank(axis=1,ascending=False)<=25
need=sorted(set(U.columns[U.any()]) | {c+'USDT' for c in ['BTC','ETH','BNB','XRP','LTC','ADA','TRX','EOS','XLM','ETC']})
print('universe ever',len(need),flush=True); json.dump(need,open('data/need_fund.json','w'))
def fund(sym):
    fn=f'data/fund2/{sym}.csv'
    if os.path.exists(fn): return
    t=int(pd.Timestamp('2019-09-01').timestamp()*1000); rows=[]; tries=0
    while True:
        x=curl(f"https://fapi.binance.com/fapi/v1/fundingRate?symbol={sym}&startTime={t}&limit=1000")
        try: d=json.loads(x)
        except Exception: d=None
        if not isinstance(d,list):
            print('retry',sym,str(x)[:100],flush=True); tries+=1
            if tries>5: break
            time.sleep(30); continue
        if not d: break
        rows+=d; t=d[-1]['fundingTime']+1
        if len(d)<1000: break
        time.sleep(0.7)
    f=pd.DataFrame(rows)
    if len(f): f['time']=pd.to_datetime(f.fundingTime,unit='ms'); f[['time','fundingRate']].to_csv(fn,index=False)
    else: pd.DataFrame(columns=['time','fundingRate']).to_csv(fn,index=False)
with cf.ThreadPoolExecutor(1) as ex: list(ex.map(fund,need))
print('funding done',flush=True)
