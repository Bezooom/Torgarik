import requests, pandas as pd, time
S=requests.Session()
def klines(base,sym,start='2017-01-01'):
    t=int(pd.Timestamp(start).timestamp()*1000); rows=[]
    while True:
        d=S.get(base,params={'symbol':sym,'interval':'1d','startTime':t,'limit':1000},timeout=30).json()
        if not isinstance(d,list) or not d: break
        rows+=d; t=d[-1][0]+86400000
        if len(d)<1000: break
    df=pd.DataFrame(rows).iloc[:,[0,4,7]] if rows else pd.DataFrame(columns=[0,4,7])
    df.columns=['t','close','qvol']; df['date']=pd.to_datetime(df.t,unit='ms').dt.date; return df[['date','close','qvol']]
coins=['BTC','ETH','BNB','XRP','LTC','ADA','TRX','EOS','XLM','ETC']
for c in coins:
    df=klines('https://api.binance.com/api/v3/klines',c+'USDT'); df.to_csv(f'data/spot_{c}.csv',index=False); print(c,'spot',len(df),df.date.min() if len(df) else None,df.date.max() if len(df) else None,flush=True)
for c in ['BTC','ETH','BNB','XRP','LTC','ADA']:
    df=klines('https://fapi.binance.com/fapi/v1/klines',c+'USDT','2019-09-01'); df.to_csv(f'data/perp_{c}.csv',index=False)
    t=int(pd.Timestamp('2019-09-01').timestamp()*1000); rows=[]
    while True:
        d=S.get('https://fapi.binance.com/fapi/v1/fundingRate',params={'symbol':c+'USDT','startTime':t,'limit':1000},timeout=30).json()
        if not isinstance(d,list) or not d: break
        rows+=d; t=d[-1]['fundingTime']+1
        if len(d)<1000: break
        time.sleep(0.3)
    f=pd.DataFrame(rows); f['time']=pd.to_datetime(f.fundingTime,unit='ms'); f[['time','fundingRate']].to_csv(f'data/funding_{c}.csv',index=False)
    print(c,'perp',len(df),'funding',len(f),f.time.min(),f.time.max(),flush=True)
