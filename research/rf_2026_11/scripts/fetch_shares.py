import os
import requests, pandas as pd, time, concurrent.futures as cf
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
def pages(url,params):
    rows=[];start=0;cols=None
    while True:
        for a in range(5):
            try: d=S.get(url,params={**params,'iss.only':'history,history.cursor','start':start},timeout=(10,30)).json();break
            except Exception: time.sleep(3)
        cols=d['history']['columns']; h=d['history']['data']; rows+=h
        idx,tot,ps=d['history.cursor']['data'][0]
        if idx+ps>=tot or not h: break
        start=idx+ps
    return pd.DataFrame(rows,columns=cols)
for ix in ['IMOEX','MCFTR']:
    if os.path.exists(f'data/{ix}.csv'): continue
    i=pages(f"https://iss.moex.com/iss/history/engines/stock/markets/index/securities/{ix}.json",{'from':'2009-01-01','history.columns':'TRADEDATE,CLOSE'})
    i.to_csv(f'data/{ix}.csv',index=False); print(ix,len(i),i.TRADEDATE.min(),i.TRADEDATE.max(),flush=True)
i=pd.read_csv('data/IMOEX.csv'); i['d']=pd.to_datetime(i.TRADEDATE)
me=i.groupby(i.d.dt.to_period('M')).d.max(); me=[d.strftime('%Y-%m-%d') for d in me if d>=pd.Timestamp('2010-01-01')]
import os
def snap(dt):
    fn=f"data/snap/{dt}.csv"
    if os.path.exists(fn): return pd.read_csv(fn)
    out=[]
    for board in ['TQBR','EQBR']:
        df=pages(f"https://iss.moex.com/iss/history/engines/stock/markets/shares/boards/{board}/securities.json",{'date':dt,'history.columns':'TRADEDATE,SECID,CLOSE,LEGALCLOSEPRICE,VALUE,VOLUME'})
        if len(df): df['board']=board; out.append(df)
    r=pd.concat(out) if out else pd.DataFrame(); r.to_csv(fn,index=False); print(dt,len(r),flush=True); return r
res=[]
with cf.ThreadPoolExecutor(3) as ex:
    for dt,df in zip(me,ex.map(snap,me)):
        df['me']=dt; res.append(df)
s=pd.concat(res); s.to_csv('data/tqbr_monthend.csv',index=False)
print(s.groupby('board').size(), s.me.min(), s.me.max(), len(me))
