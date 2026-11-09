import requests, json, os, pandas as pd, concurrent.futures as cf, time
B="https://iss.moex.com/iss/history/engines/futures/markets/forts/securities/{}.json"
MON="FGHJKMNQUVXZ"
spec={'Si':'HMUZ','RI':'HMUZ','MX':'HMUZ','GD':'HMUZ','BR':MON,'NG':MON}
import subprocess as _sp, json as _js, urllib.parse as _up
class _R:
    def __init__(s,t): s.t=t
    def json(s): return _js.loads(s.t)
class _S:
    def get(self,url,params=None,**k):
        u=url+('?'+_up.urlencode(params) if params else '')
        p=_sp.run(['curl','-s','-m','25','--connect-timeout','8',u],capture_output=True,text=True)
        if p.returncode!=0: raise Exception('curl %d'%p.returncode)
        return _R(p.stdout)
S=_S()
def get(secid):
    rows=[];start=0
    while True:
        for a in range(4):
            try:
                r=S.get(B.format(secid),params={'iss.only':'history,history.cursor','start':start,
                  'history.columns':'TRADEDATE,SECID,CLOSE,SETTLEPRICE,VOLUME,OPENPOSITION'},timeout=(10,30)); d=r.json();break
            except Exception as e: time.sleep(2)
        else: return rows
        h=d['history']['data']; rows+=h
        idx,tot,ps=d['history.cursor']['data'][0]
        if idx+ps>=tot or not h: return rows
        start=idx+ps
import os
def contract(asset,m,y):
    fn=f"data/fut/{asset}{m}{y}.json"
    if os.path.exists(fn): return asset,m,y,json.load(open(fn))
    code=f"{asset}{m}{y%10}"
    rows=get(f"{code}_{y}")
    if not rows:
        rows=get(code)
        rows=[r for r in rows if y-3<=int(r[0][:4])<=y]
    json.dump(rows,open(fn,'w')); print(asset,m,y,len(rows),flush=True)
    return asset,m,y,rows
jobs=[(a,m,y) for a,ms in spec.items() for y in range(2009,2028) for m in ms]
out=[]
with cf.ThreadPoolExecutor(3) as ex:
    for a,m,y,rows in ex.map(lambda j:contract(*j),jobs):
        for r in rows: out.append([a,f"{a}{m}{y}",MON.index(m)+1,y]+r)
df=pd.DataFrame(out,columns=['asset','contract','mon','year','date','secid','close','settle','volume','oi'])
df.to_csv('data/fut_contracts.csv',index=False); print(df.groupby('asset').agg(n=('date','size'),first=('date','min'),last=('date','max'),nc=('contract','nunique')))
