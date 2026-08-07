import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import test from 'node:test';
import {createVariantPresentation,evidenceColumnPolicy,isSpliceAiGeneField} from '../src/app/variant-presentation.js';
import {favorFieldPresentation} from '../src/app/favor-online.js';

globalThis.localStorage={getItem:()=>null,setItem:()=>{}};

test('SpliceAI recommends only the maximum score and hides its duplicate gene field',()=>{
  assert.deepEqual(evidenceColumnPolicy({sourceId:'spliceai',fieldPath:'maxDeltaScore'}),{selectable:true,recommended:true});
  for(const fieldPath of ['dsAg','dsAl','dsDg','dsDl','dpAg','dpAl','dpDg','dpDl']){
    assert.deepEqual(evidenceColumnPolicy({sourceId:'spliceai',fieldPath}),{selectable:true,recommended:false});
  }
  assert.deepEqual(evidenceColumnPolicy({sourceId:'spliceai',fieldPath:'gene'}),{selectable:false,recommended:false});
  assert.equal(isSpliceAiGeneField({sourceId:'spliceai@mane-v1.4',fieldPath:'gene'}),true);
});

const evidenceCalibrations=JSON.parse(readFileSync(new URL('../../config/evidence-calibrations.json',import.meta.url),'utf8'));
let useCalibratedEvidenceColors=true;
const text=value=>String(value??'');
const presenter=createVariantPresentation({
  $:()=>null,
  columns:{},
  escapeHtml:text,
  prototypeIcon:()=> '',
  resultFieldIdentity:field=>`${field.sourceId}.${field.fieldPath}`,
  fieldSourceIs:(item,sourceId)=>String(item?.sourceId||'').toLowerCase()===String(sourceId).toLowerCase(),
  resourceTitle:text,
  coreColumnPresentation:()=>({description:''}),
  displayColumns:()=>[],
  moveResultColumn:()=>{},
  decodeEvidenceValue:value=>value,
  resultColumnRawValue:()=>null,
  renderTableBase:()=>{},
  revealVariantDetail:()=>{},
  consequenceValue:(item,...keys)=>keys.map(key=>item?.[key]).find(value=>value!==undefined&&value!==null&&value!=='')??null,
  usefulVariantLinks:()=>[],
  displayDetailValue:text,
  dbnsfpPredictionValue:(_field,value)=>value,
  evidenceFieldPresentationBase:item=>({sourceId:item.sourceId,fieldPath:item.fieldPath,label:item.fieldPath,description:''}),
  getState:()=>({resultFieldCatalog:[],evidenceCalibrations,variants:[],resultAlignmentGroups:[],useCalibratedEvidenceColors})
});

const item=(sourceId,fieldPath,value,consequenceTerms='missense_variant')=>({sourceId,fieldPath,value,consequenceTerms});
const present=(sourceId,fieldPath,value,consequenceTerms)=>presenter.evidenceValuePresentation(item(sourceId,fieldPath,value,consequenceTerms));

test('variant details honor the backend representative when MANE Select is duplicated',()=>{
  const neighboring={feature_type:'transcript',transcript_id:'ENST_NEIGHBOR.1',mane_select:'NM_NEIGHBOR.1',consequence_terms:['downstream_gene_variant']};
  const representative={feature_type:'transcript',transcript_id:'ENST_TARGET.2',mane_select:'NM_TARGET.2',consequence_terms:['stop_gained']};
  assert.equal(presenter.preferredConsequence([neighboring,representative],'ENST_TARGET.2'),representative);
  assert.equal(presenter.preferredConsequence([neighboring,representative],'ENST_TARGET'),representative);
});

test('variant details switch transcript-scoped scalars without changing allele evidence',()=>{
  const rows=[
    {...item('dbnsfp','Ensembl_transcriptid','ENST_A'),scope:'transcript',consequenceId:'a',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','AlphaMissense_score','0.0478'),scope:'transcript',consequenceId:'a',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','REVEL_score','0.036'),scope:'transcript',consequenceId:'a',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','PrimateAI_score','0.347287714481'),scope:'transcript',consequenceId:'a',sourceCardinality:'recordScalar'},
    {...item('dbnsfp','Ensembl_transcriptid','ENST_B'),scope:'transcript',consequenceId:'b',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','AlphaMissense_score','0.0461'),scope:'transcript',consequenceId:'b',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','REVEL_score','0.036'),scope:'transcript',consequenceId:'b',sourceCardinality:'alignedVector'},
    {...item('dbnsfp','PrimateAI_score','0.347287714481'),scope:'transcript',consequenceId:'b',sourceCardinality:'recordScalar'},
    {...item('cadd','phred','20'),scope:'allele'}
  ];
  const values=(consequence,transcript)=>Object.fromEntries(
    presenter.evidenceForSelectedConsequence(rows,consequence,transcript)
      .map(row=>[`${row.sourceId}.${row.fieldPath}`,row.value])
  );
  assert.deepEqual(values('a','ENST_A'),{
    'dbnsfp.Ensembl_transcriptid':'ENST_A',
    'dbnsfp.AlphaMissense_score':'0.0478',
    'dbnsfp.REVEL_score':'0.036',
    'dbnsfp.PrimateAI_score':'0.347287714481',
    'cadd.phred':'20'
  });
  assert.deepEqual(values('b','ENST_B'),{
    'dbnsfp.Ensembl_transcriptid':'ENST_B',
    'dbnsfp.AlphaMissense_score':'0.0461',
    'dbnsfp.REVEL_score':'0.036',
    'dbnsfp.PrimateAI_score':'0.347287714481',
    'cadd.phred':'20'
  });
  const selected=presenter.evidenceForSelectedConsequence(rows,'a','ENST_A');
  assert.equal(selected.find(row=>row.fieldPath==='AlphaMissense_score').scopeLabel,'Transcript');
  const primate=selected.find(row=>row.fieldPath==='PrimateAI_score');
  assert.equal(primate.scopeLabel,'Variant');
  assert.match(presenter.evidencePresentation(primate).tooltip,/one value for this variant/i);
});

test('registered native interpretations are continuous and use semantic tones',()=>{
  const tones=new Set(['neutral','reassuring','caution','adverse']);
  for(const predictor of evidenceCalibrations.predictors.filter(predictor=>predictor.nativeInterpretation)){
    const bands=predictor.nativeInterpretation.bands;
    assert.ok(bands.length>1,`${predictor.id} has no native bands`);
    assert.equal(bands[0].minimumInclusive??bands[0].minimumExclusive,undefined,`${predictor.id} native bands do not cover low scores`);
    assert.equal(bands.at(-1).maximumInclusive??bands.at(-1).maximumExclusive,undefined,`${predictor.id} native bands do not cover high scores`);
    bands.forEach((band,index)=>{
      assert.ok(band.label,`${predictor.id} band ${index} has no label`);
      assert.ok(tones.has(band.tone),`${predictor.id} band ${index} has an invalid tone`);
      if(index===0)return;
      const previous=bands[index-1],previousLimit=previous.maximumInclusive??previous.maximumExclusive,currentLimit=band.minimumInclusive??band.minimumExclusive;
      assert.equal(previousLimit,currentLimit,`${predictor.id} native bands have a gap`);
      assert.notEqual(previous.maximumInclusive!==undefined,band.minimumInclusive!==undefined,`${predictor.id} native boundary overlaps or excludes its exact value`);
    });
  }
});

test('exact calibrated fields use semantic pills and preserve published precision gaps',()=>{
  assert.deepEqual(
    Object.fromEntries(Object.entries(present('dbnsfp','Polyphen2_HVAR_score','0.978')).filter(([key])=>['tone','presentation'].includes(key))),
    {tone:'adverse',presentation:'pill'}
  );
  assert.equal(present('dbnsfp','Polyphen2_HDIV_score','0.978').tone,'neutral');

  assert.equal(present('dbnsfp','AlphaMissense_score','0.070').presentation,'pill');
  assert.deepEqual(
    [present('dbnsfp','AlphaMissense_score','0.0705').tone,present('dbnsfp','AlphaMissense_score','0.0705').presentation],
    ['reassuring','text']
  );
  assert.equal(present('dbnsfp','AlphaMissense_score','0.071').presentation,'pill');

  assert.deepEqual(
    [present('dbnsfp','VARITY_R_score','0.0365').tone,present('dbnsfp','VARITY_R_score','0.0365').presentation],
    ['neutral','text']
  );
  assert.equal(present('dbnsfp','VARITY_R_score','0.037').presentation,'pill');
});

test('pathogenic calibrated ranges always use adverse pills',()=>{
  for(const [source,field,value,consequence] of [
    ['revel','score','0.644','missense_variant'],
    ['favor-online','codingMutPred2Score','0.737','missense_variant'],
    ['dbnsfp','SIFT_score','0.001','missense_variant'],
    ['spliceai','maxDeltaScore','0.2','intron_variant']
  ]){
    const result=present(source,field,value,consequence);
    assert.equal(result.tone,'adverse');
    assert.equal(result.presentation,'pill');
  }
});

test('SpliceAI scores and signed positions use different guidance',()=>{
  const position=presenter.evidencePresentation(item('spliceai','dpAg','-49'));
  assert.match(position.tooltip,/Signed offset in bases/);
  assert.match(position.tooltip,/Negative values are upstream/);
  assert.doesNotMatch(position.tooltip,/ranges from 0 to 1/);
  assert.equal(position.tone,'neutral');

  const score=presenter.evidencePresentation(item('spliceai','dsAg','0.82'));
  assert.match(score.tooltip,/delta score from 0 to 1/);
});

test('FAVOR MutPred2 uses calibrated bands but is excluded from summary votes',()=>{
  assert.equal(favorFieldPresentation('codingMutPred2Score','codingMutPred2Score')[0],'MutPred2 score');
  for(const [score,label] of [
    ['0.01','Strong benign'],
    ['0.0101','Moderate benign'],
    ['0.197','Moderate benign'],
    ['0.1971','Supporting benign'],
    ['0.391','Supporting benign'],
    ['0.3911','Indeterminate'],
    ['0.7369','Indeterminate'],
    ['0.737','Supporting pathogenic'],
    ['0.8289','Supporting pathogenic'],
    ['0.829','Moderate pathogenic'],
    ['0.9319','Moderate pathogenic'],
    ['0.932','Strong pathogenic']
  ])assert.match(present('favor-online','codingMutPred2Score',score).summaryNote,new RegExp(label,'i'));
  assert.deepEqual(
    [present('favor-online','codingMutPred2Score','0.391').tone,present('favor-online','codingMutPred2Score','0.391').presentation],
    ['reassuring','pill']
  );
  assert.deepEqual(
    [present('favor-online','codingMutPred2Score','0.3911').tone,present('favor-online','codingMutPred2Score','0.3911').presentation],
    ['neutral','pill']
  );
  const category=presenter.evidencePresentation(item('favor-online','codingMutPred2Pred','PP'));
  assert.equal(category.display,'Supporting pathogenic computational evidence');
  assert.equal(category.tone,'adverse');
  assert.match(presenter.predictionSummaryBar([item('favor-online','codingMutPred2Pred','PP')]),/Not reported/);
});

test('source-reported colors are exact-field plain text and remain available when calibration is off',()=>{
  assert.deepEqual(
    [present('cadd','phred','20','stop_gained').tone,present('cadd','phred','20','stop_gained').presentation],
    ['adverse','text']
  );
  assert.deepEqual(
    [present('cadd','phred','25.3').tone,present('cadd','phred','25.3').presentation],
    ['adverse','pill']
  );

  useCalibratedEvidenceColors=false;
  assert.equal(present('dbnsfp','REVEL_score','0.14').tone,'reassuring');
  assert.equal(present('dbnsfp','REVEL_score','0.5').tone,'adverse');
  assert.equal(present('dbnsfp','REVEL_score','0.75').tone,'adverse');
  assert.equal(present('dbnsfp','BayesDel_noAF_score','-0.0570106').tone,'reassuring');
  assert.equal(present('dbnsfp','BayesDel_noAF_score','-0.0570105').tone,'adverse');
  assert.equal(present('dbnsfp','PrimateAI_score','0.5999').tone,'reassuring');
  assert.equal(present('dbnsfp','PrimateAI_score','0.6').tone,'caution');
  assert.equal(present('dbnsfp','PrimateAI_score','0.8').tone,'caution');
  assert.equal(present('dbnsfp','PrimateAI_score','0.8001').tone,'adverse');
  assert.equal(present('dbnsfp','Polyphen2_HVAR_score','0.446').tone,'reassuring');
  assert.equal(present('dbnsfp','Polyphen2_HVAR_score','0.4461').tone,'caution');
  assert.equal(present('dbnsfp','Polyphen2_HVAR_score','0.908').tone,'caution');
  assert.equal(present('dbnsfp','Polyphen2_HVAR_score','0.9081').tone,'adverse');
  assert.equal(present('dbnsfp','CADD_phred','2.829').tone,'neutral');
  assert.equal(present('dbnsfp','SIFT_score','0.0499').tone,'adverse');
  assert.equal(present('dbnsfp','SIFT_score','0.05').tone,'reassuring');
  assert.equal(present('spliceai','maxDeltaScore','0.2','intron_variant').tone,'caution');
  assert.equal(present('spliceai','maxDeltaScore','0.5','intron_variant').tone,'adverse');
  assert.match(present('spliceai','maxDeltaScore','0.8','intron_variant').summaryNote,/High-precision/);
  assert.match(present('cadd','phred','30','stop_gained').summaryNote,/Top 0\.1%/);
  assert.equal(present('spliceai','maxDeltaScore','0.2','splice_donor_variant').presentation,'text');
  useCalibratedEvidenceColors=true;
});

test('display bands color non-calibrated frequency and conservation context',()=>{
  useCalibratedEvidenceColors=false;
  assert.equal(present('gnomad-exomes','AF','0.01','missense_variant').tone,'adverse');
  assert.equal(present('gnomad-exomes','AF','0.0101','missense_variant').tone,'caution');
  assert.equal(present('gnomad-exomes','AF','0.05','missense_variant').tone,'caution');
  assert.equal(present('gnomad-exomes','AF','0.0501','missense_variant').tone,'reassuring');
  assert.equal(present('favor-online','gnomadAf','0.668503','upstream_gene_variant').tone,'reassuring');
  assert.equal(present('phylop','score','-1.66','intron_variant').tone,'reassuring');
  assert.equal(present('phylop','score','0','intron_variant').tone,'reassuring');
  assert.equal(present('phylop','score','0.1','intron_variant').tone,'reassuring');
  assert.equal(present('phylop','score','1.2','intron_variant').tone,'caution');
  assert.equal(present('phylop','score','1.6','intron_variant').tone,'adverse');
  const phyloPMatch=evidenceCalibrations.predictors.find(predictor=>predictor.id==='phylop-100way-vertebrate').matches.find(match=>match.sourceIds.includes('phylop')),fieldNames=phyloPMatch.fieldNames;
  phyloPMatch.fieldNames=['score'];
  assert.equal(present('phylop','value','1.6','intron_variant').tone,'adverse');
  phyloPMatch.fieldNames=fieldNames;
  assert.equal(present('dbnsfp','phyloP100way_vertebrate','4.783129','upstream_gene_variant').tone,'adverse');
  assert.equal(present('favor-online','codingPhyloP100way','4.783129','upstream_gene_variant').tone,'adverse');
  assert.equal(present('favor-online','apcConservation','4.783129','upstream_gene_variant').tone,'reassuring');
  assert.equal(present('favor-online','apcConservation','10','upstream_gene_variant').tone,'caution');
  assert.equal(present('favor-online','apcConservation','20','upstream_gene_variant').tone,'adverse');
  assert.equal(present('favor-online','apcConservation','30','upstream_gene_variant').tone,'adverse');
  useCalibratedEvidenceColors=true;
  assert.equal(present('favor-online','caddPhred','30','missense_variant').tone,'neutral');
  assert.equal(present('dbnsfp','ESM1b_score','-24','missense_variant').tone,'neutral');
  assert.equal(present('dbnsfp','REVEL_score','.','missense_variant').tone,'missing');
});

test('population summary prefers local gnomAD and falls back to FAVOR',()=>{
  const local=item('gnomad','allAf','0.02','intron_variant');
  const online=item('favor-online','gnomadAf','0.03','intron_variant');
  assert.equal(presenter.primaryAlleleFrequency([online,local]),local);
  assert.equal(presenter.primaryAlleleFrequency([online]),online);
  assert.equal(presenter.primaryAlleleFrequency([item('gnomad','faf95','0.01','intron_variant'),online]),online);
  assert.deepEqual(favorFieldPresentation('gnomadAf','gnomadAf'),[
    'gnomAD genome + exome AF',
    'Overall allele frequency from gnomAD v4.1.1 genome and exome data represented by FAVOR.'
  ]);
});

test('variant summary prefers reported phyloP over conservation aPC',()=>{
  const apc=item('favor-online','apcConservation','24.1','intron_variant');
  const phylop=item('phylop','value','1.2','intron_variant');
  assert.equal(presenter.preferredConservationSummary([apc,phylop]),phylop);
  assert.equal(presenter.preferredConservationSummary([item('dbnsfp','phyloP100way_vertebrate','4.1','intron_variant'),phylop]),phylop);
  const legacyScopedPhyloP={...item('dbnsfp','phyloP100way_vertebrate','1.4','intron_variant'),scope:'transcript',consequenceId:'other'};
  assert.equal(presenter.preferredConservationSummary([apc,legacyScopedPhyloP]),legacyScopedPhyloP);
  const favorPhyloP=item('favor-online','codingPhyloP100way','1.3','intron_variant');
  assert.equal(presenter.preferredConservationSummary([apc,favorPhyloP]),favorPhyloP);
  assert.equal(presenter.preferredConservationSummary([apc,item('dbnsfp','phyloP100way_vertebrate_rankscore','0.99','intron_variant')]),apc);
  assert.equal(presenter.preferredConservationSummary([apc,item('phylop','score','.','intron_variant')]),apc);
});

test('missense-only scores are not interpreted for explicit non-missense consequences',()=>{
  useCalibratedEvidenceColors=false;
  const native=presenter.evidencePresentation(item('dbnsfp','REVEL_score','0','intron_variant'));
  assert.equal(native.display,'Not applicable');
  assert.equal(native.tone,'neutral');
  assert.equal(native.tier,'not-applicable');
  assert.match(native.tooltip,/applies only to missense variant/i);
  assert.doesNotMatch(native.tooltip,/No verified directional interpretation/i);
  assert.equal(present('dbnsfp','CADD_phred','20','intron_variant').display,'20');

  useCalibratedEvidenceColors=true;
  assert.equal(present('dbnsfp','REVEL_score','0','intron_variant').display,'Not applicable');
});

test('tooltips and summary use the same structured interpretation',()=>{
  const interpretation=presenter.evidencePresentation(item('revel','score','0.8'));
  assert.equal(interpretation.presentation,'pill');
  assert.match(interpretation.tooltip,/calibrated interpretation:/i);

  const summary=presenter.predictionSummaryBar([
    item('revel','score','0.8'),
    item('dbnsfp','SIFT_pred','D'),
    item('dbnsfp','Polyphen2_HVAR_pred','P'),
    item('cadd','phred','20','stop_gained')
  ]);
  assert.match(summary,/REVEL 0\.8/);
  assert.match(summary,/2 direct categorical predictors/);
  assert.doesNotMatch(summary,/3 direct categorical predictors/);
});

test('FAVOR transcript-dependent predictors remain explicit variant summaries',()=>{
  assert.equal(favorFieldPresentation('alphaMissense','alphaMissense')[0],'AlphaMissense maximum');
  assert.equal(favorFieldPresentation('revel','revel')[0],'REVEL summary');

  const alpha=presenter.evidencePresentation(item('favor-online','alphaMissense','0.4222','intron_variant'));
  assert.equal(alpha.display,'0.4222');
  assert.equal(alpha.tone,'neutral');
  assert.match(alpha.tooltip,/reports the highest AlphaMissense score/i);

  const revel=presenter.evidencePresentation(item('favor-online','revel','0.588','intron_variant'));
  assert.equal(revel.display,'0.588');
  assert.doesNotMatch(revel.tooltip,/not applicable/i);
  assert.match(revel.tooltip,/does not identify the transcript for this REVEL summary/i);

  const coding=presenter.evidencePresentation(
    item('favor-online','codingRevelScore','0.588','missense_variant'),
    '0.588'
  );
  assert.match(coding.tooltip,/FAVOR selected this coding record/i);
  assert.match(coding.tooltip,/did not match it to the selected transcript/i);
  assert.match(
    presenter.predictionSummaryBar([item('favor-online','codingSiftPred','D')]),
    /Not reported/
  );
});

test('FAVOR categorical summaries retain source-reported meaning but not MANE summary votes',()=>{
  const sift=presenter.evidencePresentation(item('favor-online','siftCat','deleterious','missense_variant'));
  assert.equal(sift.display,'Deleterious');
  assert.equal(sift.tone,'adverse');
  assert.match(sift.tooltip,/does not identify the transcript for this SIFT summary/i);

  const summary=presenter.predictionSummaryBar([
    item('favor-online','siftCat','deleterious'),
    item('favor-online','polyphenCat','probably_damaging'),
    item('favor-online','metasvmPred','D')
  ]);
  assert.match(summary,/Not reported/);
  assert.doesNotMatch(summary,/direct categorical predictors/);
});
