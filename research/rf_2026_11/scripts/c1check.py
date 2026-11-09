import pandas as pd, numpy as np, lib, warnings; warnings.filterwarnings('ignore')
exec(open('c123.py').read().split('out={}')[0])
lr=np.log(P.ffill()); score=sum((lr-lr.shift(n)).where(U).rank(axis=1) for n in (7,30,90))/3
w=xs(score,True); rets=R.fillna(0)-F.fillna(0)
for c in [0.001,0.0015,0.002]:
    o=lib.run_weights(rets,w,c,7)
    for a,b in [('2023','2024'),('2025','2026')]:
        x=o.net.loc[a:b]; print(c,a,'CAGR %.3f Sh %.2f'%((1+x).prod()**(365/len(x))-1,lib.sharpe(x,365)))
o=lib.run_weights(rets,w,0.001,7)
# contributions
tw=w.copy(); m=np.zeros(len(tw),bool); m[::7]=True
held=tw.where(pd.Series(m,index=tw.index),np.nan).ffill().fillna(0).shift(1).fillna(0).shift(1).fillna(0)
contrib=(held*rets).loc['2023':]
tot=contrib.sum().sort_values(); print('worst',tot.head(5).round(3).to_dict()); print('best',tot.tail(5).round(3).to_dict())
pr=(held*R.fillna(0)).loc['2023':].sum(axis=1).sum(); fu=(-held*F.fillna(0)).loc['2023':].sum(axis=1).sum(); print('price pnl',pr,'funding pnl',fu,'cost',o.cost.loc['2023':].sum())
x=o.net.loc['2023':]; print('drop top5 days CAGR', (1+x.drop(x.nlargest(5).index)).prod()**(365/len(x))-1)
print('long leg vs short leg 2023+:', (held.clip(lower=0)*rets).loc['2023':].sum().sum(), (held.clip(upper=0)*rets).loc['2023':].sum().sum())
print('max |R| on held', (R.abs()*(held!=0)).max().max())
