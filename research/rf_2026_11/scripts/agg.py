import pandas as pd, numpy as np, json, lib
R={}
r1=json.load(open('res_h1.json')); r2=json.load(open('res_h2.json')); rc=json.load(open('res_crypto.json'))
R['H1']=r1['H1_blend']; R['H1_sma']=r1['H1_sma']; R['H2']={k:r2[k] for k in ['IS','OOS','HOLDOUT','FULL']}
R['H3']=rc['H3_blend_longflat']; R['H3_ls']=rc['H3_blend_longshort']; R['H3_sma']=rc['H3_sma_longflat']; R['H4']=rc['H4_funding_carry']
summary={}
for k in ['H1','H2','H3','H4','H1_sma','H3_ls','H3_sma']:
    o,h=R[k]['OOS'],R[k]['HOLDOUT']; s=lib.score(o,0)
    passed = s['total']>=18 and (o['trades']+h['trades'])>=30 and o['edge']>0 and h['edge']>0
    summary[k]={'score':s,'pass':bool(passed)}
    print(k,s,'PASS' if passed else 'FAIL', 'edge OOS %.3f HO %.3f'%(o['edge'],h['edge']))
    for p in ['IS','OOS','HOLDOUT']:
        m=R[k][p]; print('   ',p,{x:(round(m[x],3) if isinstance(m[x],float) else m[x]) for x in ['start','end','CAGR','Sharpe','MaxDD','Calmar','trades','turnover_pa','cost_pa','edge','pos_years','boot5_Sharpe']})
json.dump({'R':R,'summary':summary},open('final.json','w'),indent=1,default=float)
# equity series monthly
def eq(k,ann_file):
    o=pd.read_csv(f'out_{ann_file}.csv',index_col=0,parse_dates=True); return (1+o.net).cumprod()
e1=eq('H1','H1_blend'); e2=eq('H2','H2_mom')
b=pd.read_csv('out_H2_bench.csv',index_col=0,parse_dates=True).IMOEX.fillna(0); eb=(1+b).cumprod()
m=pd.DataFrame({'H1':e1.resample('ME').last(),'H2':e2.resample('ME').last(),'IMOEX':eb.resample('ME').last()}).loc['2011-02':].ffill()
m=m/m.iloc[0]*100; print(m.iloc[::12].round(0)); m.round(1).to_csv('eq_moex.csv')
e3=eq('H3','H3_blend_longflat'); e4=eq('H4','H4_funding_carry')
c=pd.DataFrame({'H3':e3.resample('ME').last(),'H4':e4.resample('ME').last()}).loc['2019-09':].ffill(); c=c/c.iloc[0]*100
print(c.iloc[::12].round(0)); c.round(1).to_csv('eq_crypto.csv')
