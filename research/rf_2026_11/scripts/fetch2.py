import subprocess, json, re, os, pandas as pd, concurrent.futures as cf, time
def curl(u,t=30):
    for a in range(4):
        p=subprocess.run(['curl','-s','-m',str(t),'--connect-timeout','8',u],capture_output=True,text=True)
        if p.returncode==0 and p.stdout: return p.stdout
        time.sleep(2)
    return None
syms=[];marker=''
while True:
    x=curl(f"https://s3-ap-northeast-1.amazonaws.com/data.binance.vision?prefix=data/futures/um/monthly/klines/&delimiter=/&marker={marker}")
    s=re.findall(r'<Prefix>data/futures/um/monthly/klines/([^/<]+)/</Prefix>',x); syms+=s
    m=re.search(r'<NextMarker>([^<]+)</NextMarker>',x)
    if not m or 'true' not in x[:600]: break
    marker=m.group(1)
EX=('USDC','BUSD','TUSD','FDUSD','USDP','DAI','EUR','GBP')
syms=sorted(set(s for s in syms if s.endswith('USDT') and s[:-4] not in EX and '_' not in s))
print('symbols',len(syms),flush=True); json.dump(syms,open('data/syms.json','w'))
def kl(sym):
    fn=f'data/perp2/{sym}.csv'
    if os.path.exists(fn): return
    t=int(pd.Timestamp('2019-09-01').timestamp()*1000); rows=[]
    while True:
        x=curl(f"https://fapi.binance.com/fapi/v1/klines?symbol={sym}&interval=1d&startTime={t}&limit=1500")
        try: d=json.loads(x)
        except Exception: d=[]
        if not isinstance(d,list) or not d: break
        rows+=d; t=d[-1][0]+86400000
        if len(d)<1500: break
    df=pd.DataFrame([[r[0],float(r[4]),float(r[7])] for r in rows],columns=['t','close','qvol'])
    df['date']=pd.to_datetime(df.t,unit='ms').dt.date; df[['date','close','qvol']].to_csv(fn,index=False)
with cf.ThreadPoolExecutor(6) as ex: list(ex.map(kl,syms))
print('klines done',flush=True)
