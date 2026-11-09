import subprocess, json, os, io, zipfile, pandas as pd, concurrent.futures as cf, time, urllib.request
need=json.load(open('data/need_fund.json'))
P=pd.read_pickle('data/P2.pkl')
jobs=[]
for s in need:
    if os.path.exists(f'data/fund2/{s}.csv'): continue
    st=P[s].first_valid_index(); en=P[s].last_valid_index()
    for m in pd.period_range(st.to_period('M'),min(en,pd.Timestamp('2026-10-31')).to_period('M'),freq='M'): jobs.append((s,str(m)))
print('jobs',len(jobs),flush=True)
def get(j):
    s,m=j; u=f"https://data.binance.vision/data/futures/um/monthly/fundingRate/{s}/{s}-fundingRate-{m}.zip"
    for a in range(3):
        p=subprocess.run(['curl','-s','-f','-m','30',u],capture_output=True)
        if p.returncode==0:
            z=zipfile.ZipFile(io.BytesIO(p.stdout)); t=z.read(z.namelist()[0]).decode()
            return s,t
        if p.returncode==22: return s,None
        time.sleep(1)
    return s,'ERR'
from collections import defaultdict
acc=defaultdict(list); err=0; miss=0
with cf.ThreadPoolExecutor(16) as ex:
    for i,(s,t) in enumerate(ex.map(get,jobs)):
        if t is None: miss+=1
        elif t=='ERR': err+=1
        else:
            lines=[l for l in t.strip().splitlines() if l and l[0].isdigit()]
            acc[s]+=[l.split(',')[:3] for l in lines]
        if i%2000==0: print(i,flush=True)
print('miss',miss,'err',err,flush=True)
for s in need:
    if os.path.exists(f'data/fund2/{s}.csv'): continue
    rows=acc.get(s,[])
    f=pd.DataFrame(rows,columns=['fundingTime','interval','fundingRate'])
    f['time']=pd.to_datetime(f.fundingTime.astype('int64'),unit='ms'); f['fundingRate']=f.fundingRate.astype(float)
    # append Nov-2026 via REST
    try:
        x=subprocess.run(['curl','-s','-m','20',f"https://fapi.binance.com/fapi/v1/fundingRate?symbol={s}&startTime={int(pd.Timestamp('2026-11-01').timestamp()*1000)}&limit=1000"],capture_output=True,text=True).stdout
        d=json.loads(x)
        if isinstance(d,list) and d:
            g=pd.DataFrame(d); g['time']=pd.to_datetime(g.fundingTime,unit='ms'); g['fundingRate']=g.fundingRate.astype(float); f=pd.concat([f,g])
    except Exception: pass
    time.sleep(0.25)
    f[['time','fundingRate']].sort_values('time').to_csv(f'data/fund2/{s}.csv',index=False)
print('done',flush=True)
