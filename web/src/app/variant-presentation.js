export function applyGenericEvidenceCellPresentation(cell, html) {
  if (cell.querySelector('.profile-evidence-cell')) return false;
  cell.innerHTML = html;
  return true;
}

export function evidenceColumnPolicy(field) {
  const source=String(field?.sourceId||'').toLowerCase();
  const leaf=String(field?.fieldPath||'').split(/[.\[\]]/).filter(Boolean).pop()?.replace(/[^a-z0-9]/gi,'').toLowerCase()||'';
  if(source==='spliceai'||source.startsWith('spliceai-')||source.startsWith('spliceai@')){
    if(leaf==='gene')return{selectable:false,recommended:false};
    if(leaf==='maxdeltascore')return{selectable:true,recommended:true}
  }
  return{selectable:field?.selectable!==false,recommended:false}
}

export function createVariantPresentation({
  $,columns,escapeHtml,prototypeIcon,resultFieldIdentity,fieldSourceIs,resourceTitle,
  coreColumnPresentation,displayColumns,moveResultColumn,decodeEvidenceValue,resultColumnRawValue,
  renderTableBase,revealVariantDetail,consequenceValue,usefulVariantLinks,displayDetailValue,
  dbnsfpPredictionValue,evidenceFieldPresentationBase,getState
}){
  let resultFieldCatalog=[],evidenceCalibrations={interpretationPolicy:{},predictors:[],calibrations:[],displayPolicies:{}},variants=[],resultAlignmentGroups=[],useCalibratedEvidenceColors=true;
  function syncState(){({resultFieldCatalog,evidenceCalibrations,variants,resultAlignmentGroups,useCalibratedEvidenceColors=true}=getState())}

const detailConsequenceSelections=new Map();
const variantDetailOpenSections=new Set(['clinical-population']);
const resultGridWidths=new Map((()=>{try{return Object.entries(JSON.parse(localStorage.getItem('annocat.resultColumnWidths')||'{}'))}catch{return[]}})());
let resultGridViewportObserver=null,resultGridViewportTarget=null;
const dbnsfpTranscriptMetadata=new Set(['Ensembl_geneid','Ensembl_transcriptid','Ensembl_proteinid','Uniprot_acc','HGVSc_VEP','HGVSp_VEP','APPRIS','GENCODE_basic','TSL','VEP_canonical','aaref','aaalt','aapos','genename']);
const favorTranscriptSummaries=new Map([
  ['revel',{family:'revel',note:'FAVOR does not identify the transcript for this REVEL summary.'}],
  ['alphamissense',{family:'alphamissense',note:'FAVOR reports the highest AlphaMissense score. FAVOR does not identify the transcript.'}],
  ['siftcat',{family:'sift',note:'FAVOR does not identify the transcript for this SIFT summary.'}],
  ['polyphencat',{family:'polyphen',note:'FAVOR does not identify the transcript for this PolyPhen-2 summary.'}],
  ['metasvmpred',{family:'metasvm',note:'FAVOR does not identify the transcript for this MetaSVM summary.'}]
]);
function dbnsfpVariantLevelField(field){return/^(CADD_|GERP\+\+_|phyloP|phastCons|SiPhy|LINSIGHT|GenoCanyon|fitCons|Eigen)/i.test(String(field||''))}
function favorTranscriptSummary(item){
  if(!fieldSourceIs(item,'favor-online'))return null;
  return favorTranscriptSummaries.get(evidenceFieldLeaf(item).toLowerCase())||null
}
function favorSourceSelectedCoding(item){
  return fieldSourceIs(item,'favor-online')&&evidenceFieldLeaf(item).toLowerCase().startsWith('coding')
}
function calibrationSourceVerified(match){return(match?.calibrationVerificationStatus||match?.verificationStatus)==='approved'}
function nativeSourceVerified(match){return(match?.nativeVerificationStatus||match?.verificationStatus)==='approved'}

function evidenceReadingGuide(source,field){
  const id=String(source||'').toLowerCase(),key=String(field||''),lower=key.toLowerCase(),normalized=lower.replace(/[^a-z0-9]/g,'');
  const predictor=calibratedPredictorDefinition({sourceId:source,fieldPath:field});
  const category=categoricalPredictionDefinition({sourceId:source,fieldPath:field});
  if(category)return category.note||'Prediction reported by dbNSFP.';
  if(predictor?.calibrationStatus==='published'&&calibrationSourceVerified(predictor.sourceMatch))return`${predictor.scoreIdentity}. Published thresholds apply when they support this variant type. Otherwise, AnnoCAT uses the source threshold.`;
  if(predictor?.nativeInterpretation&&nativeSourceVerified(predictor.sourceMatch))return`${predictor.scoreIdentity}. AnnoCAT uses the threshold from the data source. The source release is not verified for published calibration.`;
  if(predictor)return`${predictor.scoreIdentity}. No verified threshold is available. AnnoCAT does not assign a direction.`;
  if(lower.includes('apc')&&lower.includes('protein'))return'FAVOR provides this PHRED-ranked protein-function summary. Scores of 10, 20, and 30 approximate the top 10%, 1%, and 0.1%. Higher scores suggest stronger functional effects.';
  if(lower.includes('apc')&&lower.includes('conservation'))return'FAVOR provides this PHRED-ranked conservation summary. Scores of 10, 20, and 30 approximate the top 10%, 1%, and 0.1%. Higher scores suggest stronger evolutionary constraint.';
  if(lower.includes('apc')&&(lower.includes('epigen')||lower.includes('transcription_factor')||lower.includes('transcription-factor')))return'FAVOR provides this PHRED-ranked regulatory-context summary. Higher scores suggest stronger biological activity in the surrounding region. They do not prove an allele-specific effect.';
  if(lower.includes('apc')&&lower.includes('mappability'))return'FAVOR summary of sequence uniqueness. Interpret this as technical context rather than pathogenicity evidence.';
  if(lower.includes('apc')&&(lower.includes('proximity')||lower.includes('distance')))return'FAVOR proximity summary. Nearby genes are candidates, but the nearest gene is not necessarily the affected gene.';
  if(lower.includes('apc')&&(lower.includes('mutation')||lower.includes('diversity')||lower.includes('microrna')))return'FAVOR regional-context summary. Higher values indicate a stronger signal in this category, not pathogenicity by themselves.';
  if(lower.includes('mappability')||lower.includes('umap')||lower.includes('bismap'))return'This sequence-uniqueness measure ranges from 0 to 1. Values near 1 are easier to map. Review low values with read and mapping quality.';
  if(lower.includes('min_dist_tss'))return'Distance in bases to the nearest transcription start site. Smaller values mean closer, but proximity alone does not establish a target gene.';
  if(lower.includes('min_dist_tse')||lower.includes('min_dist_tes'))return'Distance in bases to the nearest transcription end site. Smaller values mean closer.';
  if(lower.includes('remap')&&lower.includes('overlap_tf'))return'Number of transcription factors observed binding at this location. More overlap suggests regulatory activity, but tissue and allele-specific effects still matter.';
  if(lower.includes('remap')&&lower.includes('overlap_cl'))return'Number of transcription-factor and cell-line combinations observed at this location. A larger count indicates broader experimental overlap.';
  if(lower.includes('ccre'))return'ENCODE candidate regulatory-element annotation. It describes the surrounding region and does not prove that this allele changes its activity.';
  if(lower.includes('chromhmm')||lower.includes('chromatin'))return'Chromatin-state evidence comes from assayed cells or tissues. Its relevance depends on the study context.';
  if(lower.includes('sift_score'))return'The score ranges from 0 to 1. Scores at or below 0.05 suggest a deleterious effect.';
  if(lower.includes('sift_pred'))return'D means deleterious. T means tolerated.';
  if(lower.includes('polyphen')&&lower.includes('score'))return'The score ranges from 0 to 1. Higher scores suggest a damaging protein effect.';
  if(lower.includes('polyphen')&&lower.includes('pred'))return'D means probably damaging. P means possibly damaging. B means benign.';
  if(lower.includes('alphamissense_score')||id.includes('alphamissense')&&lower.includes('score'))return'The score ranges from 0 to 1. Scores below 0.34 are typically benign. Scores above 0.56 are typically pathogenic.';
  if(lower.includes('alphamissense_pred')||id.includes('alphamissense')&&lower.includes('pred'))return'B means benign. A means uncertain. P means pathogenic.';
  if(lower.includes('revel')||id.includes('revel')&&lower.includes('score'))return'The score ranges from 0 to 1. Higher scores suggest pathogenicity. Thresholds depend on the intended use.';
  if((lower.includes('cadd')||id.includes('cadd'))&&lower.includes('phred'))return'Higher scores suggest a more deleterious effect. Scores of 10 and 20 approximate the top 10% and 1%.';
  if(lower.includes('primateai_score')||id.includes('primateai')&&lower.includes('score'))return'The original PrimateAI score ranges from 0 to 1. Higher scores suggest a damaging protein effect.';
  if(lower.includes('primateai_pred')||id.includes('primateai')&&lower.includes('pred'))return'dbNSFP uses 0.803 as the cutoff between D and T.';
  if(id.includes('spliceai')&&['dpag','dpal','dpdg','dpdl'].includes(normalized))return'Signed offset in bases from the variant. Negative values are upstream; positive values are downstream.';
  if(id.includes('spliceai')&&normalized==='maxdeltascore')return'Maximum of the four SpliceAI delta scores. The score ranges from 0 to 1.';
  if((id.includes('spliceai')&&['dsag','dsal','dsdg','dsdl'].includes(normalized))||lower.startsWith('ds_'))return'SpliceAI delta score from 0 to 1.';
  if(id.includes('phylop')||lower.includes('phylop'))return'Positive values indicate conservation. Negative values indicate accelerated change.';
  if(id.includes('gerp')||lower.includes('gerp'))return'Higher positive values indicate stronger constraint. Values above 2 often indicate conservation.';
  if(lower==='af'||lower.includes('allele_frequency'))return'Lower population frequencies are rarer. Rarity alone does not indicate pathogenicity.';
  if(lower.includes('significance'))return'Interpret with ClinVar review status and supporting submissions.';
  if(lower==='tsl')return'Level 1 has the strongest transcript support. No value means that the source did not report a level.';
  if(lower==='appris')return'Principal identifies a preferred protein isoform. Alternative identifies another supported isoform.';
  return'';
}

function evidenceFieldPresentation(field){
  const presentation=evidenceFieldPresentationBase(field),guide=evidenceReadingGuide(presentation.sourceId,presentation.fieldPath);
  return{...presentation,baseDescription:presentation.description,readingGuide:guide,description:[presentation.description,guide].filter(Boolean).join(' ')};
};

function readableTerm(value){return String(value??'').replace(/_/g,' ').replace(/\s+/g,' ').trim()}
function consequenceTerms(value){
  const values=Array.isArray(value)?value:String(value??'').split(/[,&|;]/);
  return[...new Set(values.map(item=>String(item||'').trim()).filter(Boolean))]
}
function primaryConsequence(value){return consequenceTerms(value)[0]||''}
function additionalConsequences(value){return consequenceTerms(value).slice(1)}
function phredRankLabel(number){
  if(!Number.isFinite(number)||number<0)return'';
  const percentage=100*Math.pow(10,-number/10),digits=percentage>=10?1:percentage>=1?2:percentage>=.1?3:4;
  return`top ${percentage.toFixed(digits).replace(/\.0+$/,'').replace(/(\.\d*?)0+$/,'$1')}%`;
}
function plainDecimalNotation(value){
  const text=String(value??'').trim(),match=text.match(/^([+-]?)(\d+)(?:\.(\d*))?[eE]([+-]?\d+)$/);
  if(!match)return text;
  const sign=match[1],whole=match[2],fraction=match[3]||'',digits=whole+fraction,point=whole.length+Number(match[4]);
  const expanded=point<=0?`0.${'0'.repeat(-point)}${digits}`:point>=digits.length?`${digits}${'0'.repeat(point-digits.length)}`:`${digits.slice(0,point)}.${digits.slice(point)}`;
  return`${sign}${expanded}`.replace(/(\.\d*?)0+$/,'$1').replace(/\.$/,'')
}
function conciseNumericNotation(value,significantDigits=7){
  const number=Number(value);
  if(!Number.isFinite(number))return String(value??'');
  if(number===0)return'0';
  return plainDecimalNotation(Number(number.toPrecision(significantDigits)).toString())
}
function evidenceResult(display,tone='neutral',tier='neutral',presentation='text',details={}){
  return{display,tone,tier,presentation,...details}
}
function evidenceClass(result){
  return`tone-${escapeHtml(result?.tone||'neutral')} evidence-presentation-${escapeHtml(result?.presentation||'text')}`
}
function directionTone(direction){
  return direction==='pathogenic'?'adverse':direction==='benign'?'reassuring':'neutral'
}
function scoreBand(bands,score){
  if(!Number.isFinite(score))return null;
  const within=(band,key,comparison)=>band[key]===undefined||comparison(score,Number(band[key]));
  return(bands||[]).find(band=>within(band,'minimumInclusive',(value,limit)=>value>=limit)&&within(band,'minimumExclusive',(value,limit)=>value>limit)&&within(band,'maximumInclusive',(value,limit)=>value<=limit)&&within(band,'maximumExclusive',(value,limit)=>value<limit))||null
}
function evidenceFieldLeaf(item){return String(item?.fieldPath||'').split(/[.\[\]]/).filter(Boolean).pop()||''}
function calibratedPredictorDefinition(item){
  const field=evidenceFieldLeaf(item).toLowerCase(),fields=fieldSourceIs(item,'phylop')&&field==='value'?new Set(['value','score']):new Set([field]);
  for(const predictor of evidenceCalibrations.predictors||[]){
    const sourceMatch=(predictor.matches||[]).find(match=>(match.sourceIds||[]).some(sourceId=>fieldSourceIs(item,sourceId))&&(match.fieldNames||[]).some(fieldName=>fields.has(String(fieldName).toLowerCase())));
    if(sourceMatch)return{...predictor,sourceMatch}
  }
  return null
}
function categoricalPredictionDefinition(item){
  const field=evidenceFieldLeaf(item).toLowerCase();
  return(evidenceCalibrations.categoricalPredictions||[]).find(definition=>fieldSourceIs(item,definition.sourceId)&&String(definition.fieldName||'').toLowerCase()===field)||null
}
function categoricalPredictionPresentation(item,raw){
  const definition=categoricalPredictionDefinition(item);
  if(!definition)return null;
  const values=String(raw??'').split(/[,|&]/).map(value=>value.trim()).filter(value=>value&&value!=='.'&&value!=='-');
  if(!values.length)return evidenceResult('Not reported','missing','missing');
  const codes=new Map((definition.codes||[]).map(code=>[String(code.value).toLowerCase(),code])),mapped=values.map(value=>codes.get(value.toLowerCase())).filter(Boolean);
  if(mapped.length!==values.length)return null;
  const labels=[...new Set(mapped.map(code=>code.label))],tones=new Set(mapped.map(code=>code.tone));
  return evidenceResult(labels.join(', '),tones.size===1?mapped[0].tone:'neutral','source-native','text',{
    summaryNote:definition.note||'This source uses a native categorical interpretation.',
    categoryInterpretation:{fieldName:definition.fieldName,displayOnly:true,note:definition.note||''}
  })
}
function normalizedConsequenceTerms(value){
  const values=Array.isArray(value)?value:[value];
  return new Set(values.flatMap(item=>String(item||'').split(/[,&|;]/)).map(item=>item.trim().toLowerCase().replace(/\s+/g,'_')).filter(Boolean))
}
function predictorApplicability(predictor,item){
  const required=predictor?.variantClasses||[],excluded=predictor?.excludedVariantClasses||[],observed=normalizedConsequenceTerms(item?.consequenceTerms);
  if(excluded.length){
    if(!observed.size)return{applies:false,reason:'missing-context',note:`This predictor excludes ${excluded.map(readableTerm).join(', ')}, but no selected consequence context was available.`};
    const matched=excluded.filter(term=>observed.has(String(term).toLowerCase()));
    if(matched.length)return{applies:false,reason:'excluded-variant-class',note:`This predictor is not interpreted for ${matched.map(readableTerm).join(', ')}; canonical splice-site variants require separate loss-of-function assessment.`}
  }
  if(!required.length||required.includes('any'))return{applies:true,reason:'',note:''};
  if(!observed.size)return{applies:false,reason:'missing-context',note:`This predictor applies only to ${required.map(readableTerm).join(', ')}, but no selected consequence context was available.`};
  if(required.some(term=>observed.has(String(term).toLowerCase())))return{applies:true,reason:'',note:''};
  return{applies:false,reason:'incompatible-variant-class',note:`This predictor applies only to ${required.map(readableTerm).join(', ')} and is not interpreted for ${[...observed].map(readableTerm).join(', ')}.`}
}
function calibratedEvidenceBand(calibrationId,score){
  const calibration=(evidenceCalibrations.calibrations||[]).find(item=>item.id===calibrationId);
  if(!calibration||!Number.isFinite(score))return null;
  const band=scoreBand(calibration.bands,score);
  return band?{...band,calibration:calibration.reference,referenceUrl:calibration.referenceUrl,calibrationScope:calibration.scope,geneSpecific:Boolean(calibration.geneSpecific),singlePredictorOnly:Boolean(calibration.singlePredictorOnly)}:null
}
function calibrationThresholdLabel(band){
  if(!band)return'';
  const limits=[];
  if(band.minimumInclusive!==undefined)limits.push(`≥ ${plainDecimalNotation(band.minimumInclusive)}`);
  if(band.minimumExclusive!==undefined)limits.push(`> ${plainDecimalNotation(band.minimumExclusive)}`);
  if(band.maximumInclusive!==undefined)limits.push(`≤ ${plainDecimalNotation(band.maximumInclusive)}`);
  if(band.maximumExclusive!==undefined)limits.push(`< ${plainDecimalNotation(band.maximumExclusive)}`);
  return limits.join(' and ')
}
function conciseEvidenceBandLabel(label){
  return String(label||'').replace('Indeterminate calibrated range','No calibrated direction').replace(/ computational evidence/gi,' evidence')
}
function calibratedPredictorInterpretation(item,score){
  const predictor=calibratedPredictorDefinition(item);
  if(!predictor)return null;
  const applicability=predictorApplicability(predictor,item);
  if(predictor.calibrationStatus!=='published')return{predictor,applicable:false,note:'No published calibration was applied because the score identity has not been verified.'};
  if(!calibrationSourceVerified(predictor.sourceMatch))return{predictor,applicable:false,note:predictor.sourceMatch?.reason||'No published calibration was applied because this source match has not been verified.'};
  if(!applicability.applies)return{predictor,applicable:false,note:applicability.note};
  const evidenceBand=calibratedEvidenceBand(predictor.calibrationId,score);
  return{predictor,applicable:true,evidenceBand:evidenceBand?{...evidenceBand,predictorId:predictor.id,predictorLabel:predictor.label,evidenceGroup:predictor.evidenceGroup,role:predictor.role,scoreIdentity:predictor.scoreIdentity}:null,note:evidenceBand?'':'The score did not fall in a published calibrated interval.'}
}
function nativePredictorInterpretation(item,score){
  const predictor=calibratedPredictorDefinition(item),policy=predictor?.nativeInterpretation;
  if(!policy||!nativeSourceVerified(predictor.sourceMatch))return null;
  const applicability=predictorApplicability(policy,item);
  if(!applicability.applies)return null;
  const band=scoreBand(policy.bands,score);
  return band?{predictor,policy,band}:null
}
function alleleFrequencyField(source,field){
  const id=String(source||'').toLowerCase(),lower=String(field||'').toLowerCase(),compact=lower.replace(/[^a-z0-9]/g,''),population='all|global|grpmax|afr|amr|asj|eas|fin|mid|nfe|oth|remaining|sas|exac|tgp|esp\\d*|1000g';
  return['gnomadaf','bravoaf','tgall'].includes(compact)||lower==='af'||lower.includes('allele_frequency')||/(^|[._])faf\d*([._]|$)/.test(lower)||new RegExp(`(${population})_?af$`).test(lower)||new RegExp(`(^|[._])af[._]?(${population})$`).test(lower)||(id.includes('gnomad')&&/(^|[._])af([._]|$)/.test(lower))
}
function alleleFrequencyTone(value){
  const text=String(value??'').trim(),lower=text.toLowerCase();
  if(!text||text==='.'||text==='-'||/not available|not reported/.test(lower))return'missing';
  const number=Number(text.match(/-?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?/i)?.[0]);
  if(!Number.isFinite(number)||number<0||number>1)return'neutral';
  return scoreBand(evidenceCalibrations.displayPolicies?.alleleFrequency?.bands,number)?.tone||'neutral'
}
function alleleFrequencySummary(value){
  const number=Number(String(value??'').trim());
  const band=Number.isFinite(number)&&number>=0&&number<=1?scoreBand(evidenceCalibrations.displayPolicies?.alleleFrequency?.bands,number):null;
  return band?`${band.label} (${calibrationThresholdLabel(band)}).`:''
}
function conservationApcPresentation(item,number,display){
  const field=evidenceFieldLeaf(item).toLowerCase().replace(/[^a-z0-9]/g,'');
  if(!fieldSourceIs(item,'favor-online')||field!=='apcconservation')return null;
  const band=scoreBand(evidenceCalibrations.displayPolicies?.conservation?.apcBands,number);
  return band?evidenceResult(display,band.tone,'source-native','text',{summaryNote:`${band.label} (${calibrationThresholdLabel(band)}).`}):null
}
function primaryAlleleFrequency(items){
  const frequencies=items.filter(item=>evidenceDomain(item)==='population'&&alleleFrequencyField(item.sourceId,item.fieldPath));
  const priority=item=>{
    const source=String(item.sourceId||'').toLowerCase(),field=String(item.fieldPath||'').toLowerCase().replace(/[^a-z0-9]/g,''),sourceRank=fieldSourceIs(item,'favor-online')?1:source.includes('gnomad')?0:2;
    if(field==='allaf'||field==='af'||field==='allelefrequency'||field==='gnomadaf')return sourceRank;
    if(field==='globalmaf')return 10+sourceRank;
    if(field==='grpmaxaf'||field.includes('maxfrequency'))return 20+sourceRank;
    if(field==='faf95')return 30+sourceRank;
    if(field==='faf99')return 40+sourceRank;
    return 90+sourceRank
  };
  return[...frequencies].sort((left,right)=>priority(left)-priority(right))[0]||null
}
function groupMaximumAlleleFrequency(items){const rank=item=>{const field=String(item.fieldPath||'').toLowerCase().replace(/[^a-z0-9]/g,'');if(field.includes('grpmaxaf')||field.includes('groupmaxaf')||field.includes('popmaxaf'))return 0;if(field.includes('maxfrequency')||field.includes('maxaf'))return 1;return 10};return items.filter(item=>evidenceDomain(item)==='population'&&alleleFrequencyField(item.sourceId,item.fieldPath)&&rank(item)<10).sort((left,right)=>rank(left)-rank(right))[0]||null}
function conservationSummaryPriority(item){const source=String(item.sourceId||'').toLowerCase(),field=evidenceFieldLeaf(item).toLowerCase().replace(/[^a-z0-9]/g,'');if(field.includes('rank'))return 10;if(source.includes('phylop')&&['score','value'].includes(field))return 0;if(!source.includes('favor')&&field.includes('phylop'))return 1;if(field.includes('phylop'))return 2;if(field==='apcconservation')return 3;return 10}
function preferredConservationSummary(items){return[...items].filter(item=>conservationSummaryPriority(item)<10&&evidenceValuePresentation(item).tone!=='missing').sort((left,right)=>conservationSummaryPriority(left)-conservationSummaryPriority(right))[0]||null}
function reportHasFrequencyFields(){return resultFieldCatalog.some(field=>evidenceDomain(field)==='population'&&alleleFrequencyField(field.sourceId,field.fieldPath))}
function sortPopulationEvidence(items){
  const rank=item=>{
    const source=String(item.sourceId||'').toLowerCase(),rawField=String(item.fieldPath||'').toLowerCase(),field=rawField.replace(/[^a-z0-9]/g,''),sourceRank=source.includes('clinvar')?100:0;
    if(/^(af|allaf|globalaf|overallaf|allelefrequency)$/.test(field))return sourceRank;
    if(/grpmaxaf|groupmaxaf|popmaxaf|maxfrequency/.test(field))return sourceRank+1;
    if(/^(ac|allelecount)$/.test(field))return sourceRank+2;
    if(/^(an|allelenumber)$/.test(field))return sourceRank+3;
    if(/nhomalt|homozyg/.test(field))return sourceRank+4;
    if(alleleFrequencyField(source,rawField))return sourceRank+10;
    return sourceRank+20
  };
  return[...items].sort((left,right)=>rank(left)-rank(right)||evidenceFieldPresentation(left).label.localeCompare(evidenceFieldPresentation(right).label))
}
const populationAncestryLabels={
  afr:'African/African American',
  ami:'Amish',
  amr:'Admixed American',
  asj:'Ashkenazi Jewish',
  eas:'East Asian',
  fin:'Finnish',
  mid:'Middle Eastern',
  nfe:'European (non-Finnish)',
  oth:'Other ancestry group',
  remaining:'Remaining individuals',
  sas:'South Asian'
};
function populationFieldKey(item){
  const path=String(item?.fieldPath||''),leaf=path.split(/[.\[\]]/).filter(Boolean).pop()||path;
  return leaf.toLowerCase().replace(/[^a-z0-9]/g,'')
}
function populationFieldKind(item){
  const field=populationFieldKey(item);
  if(/^(allaf|af|overallaf|globalaf|allelefrequency|gnomadaf)$/.test(field))return'overall';
  if(/grpmaxaf|groupmaxaf|popmaxaf|maxfrequency/.test(field))return'groupMaximum';
  if(/grpmaxpopulation|groupmaxpopulation|popmaxpopulation|maxfrequencygroup/.test(field))return'groupMaximumLabel';
  if(/^(allac|ac|allelecount)$/.test(field))return'alleleCount';
  if(/^(allan|an|allelenumber)$/.test(field))return'alleleNumber';
  if(/^(allhc|nhomalt|homozygotecount|homozygouscount)$/.test(field)||/nhomalt|homozyg/.test(field))return'homozygotes';
  return''
}
function populationAncestryEntry(item){
  const field=populationFieldKey(item),match=field.match(/^(afr|ami|amr|asj|eas|fin|mid|nfe|oth|remaining|sas)af$/)||field.match(/^af(afr|ami|amr|asj|eas|fin|mid|nfe|oth|remaining|sas)$/);
  return match?{code:match[1],label:populationAncestryLabels[match[1]]}:null
}
function populationGroupCode(value){
  const normalized=String(value??'').toLowerCase().replace(/[^a-z0-9]/g,'');
  if(populationAncestryLabels[normalized])return normalized;
  return Object.entries(populationAncestryLabels).find(([,label])=>label.toLowerCase().replace(/[^a-z0-9]/g,'')===normalized)?.[0]||''
}
function populationCountPresentation(item){
  if(!item)return{display:'Not reported',tone:'missing'};
  const interpreted=evidenceValuePresentation(item),number=Number(decodeEvidenceValue(item.value));
  return Number.isInteger(number)&&number>=0?{...interpreted,display:number.toLocaleString()}:interpreted
}
function populationSummaryMetric(label,presentation,tooltip){
  return`<div class="annotation-row population-summary-row fui-key-value-row tone-${escapeHtml(presentation.tone||'neutral')}" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><span class="annotation-field"><strong>${escapeHtml(label)}</strong></span><b>${escapeHtml(displayDetailValue(presentation.display))}</b></div>`
}
function populationEvidenceRow(item,label,isMaximum=false,showSource=false){
  const interpreted=evidencePresentation(item,item.value,{includeSource:true}),tooltip=interpreted.tooltip,sourcePrefix=showSource?`${resourceTitle(item.sourceId)} · `:'';
  return`<div class="annotation-row fui-key-value-row tone-${interpreted.tone}${isMaximum?' group-maximum-row':''}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(`${sourcePrefix}${label}`)}</strong>${isMaximum?'<small>Highest</small>':''}</span><b>${escapeHtml(interpreted.display)}</b></div>`
}
function populationEvidenceSubgroup(items,preferredSourceId='',empty='Not reported'){
  if(!items.length)return`<div class="key-evidence-subgroup population-evidence" data-evidence-group="population"><div class="key-evidence-subheading fui-section-heading"><strong>Population</strong></div><div class="annotation-list fui-key-value-list"><div class="key-evidence-empty">${escapeHtml(empty)}</div></div></div>`;
  const sourceKey=value=>String(value||'').toLowerCase(),preferredKey=sourceKey(preferredSourceId),primarySourceId=items.find(item=>sourceKey(item.sourceId)===preferredKey)?.sourceId||items.find(item=>sourceKey(item.sourceId).includes('gnomad'))?.sourceId||items[0].sourceId,primaryKey=sourceKey(primarySourceId),primaryItems=items.filter(item=>sourceKey(item.sourceId)===primaryKey);
  const findKind=kind=>primaryItems.find(item=>populationFieldKind(item)===kind),overall=findKind('overall'),alleleCount=findKind('alleleCount'),alleleNumber=findKind('alleleNumber'),homozygotes=findKind('homozygotes'),reported=item=>item&&evidenceValuePresentation(item).tone!=='missing';
  const ancestryRows=primaryItems.map(item=>{const ancestry=populationAncestryEntry(item);if(!ancestry)return null;const number=Number(decodeEvidenceValue(item.value));return{...ancestry,item,number:Number.isFinite(number)&&number>=0&&number<=1?number:Number.NEGATIVE_INFINITY}}).filter(Boolean).sort((left,right)=>right.number-left.number||left.label.localeCompare(right.label));
  const explicitMaximum=findKind('groupMaximum'),maximumLabelItem=findKind('groupMaximumLabel'),derivedMaximum=ancestryRows.find(row=>Number.isFinite(row.number)&&row.number!==Number.NEGATIVE_INFINITY),maximumItem=reported(explicitMaximum)?explicitMaximum:derivedMaximum?.item,maximumPresentation=maximumItem?evidenceValuePresentation(maximumItem):{display:'Not reported',tone:'missing'},maximumCode=populationGroupCode(decodeEvidenceValue(maximumLabelItem?.value))||derivedMaximum?.code||'',maximumLabel=maximumCode?populationAncestryLabels[maximumCode]:maximumLabelItem?readableTerm(displayDetailValue(maximumLabelItem.value)):'';
  const overallPresentation=overall?evidenceValuePresentation(overall):{display:'Not reported',tone:'missing'},countPresentation=populationCountPresentation(alleleCount),numberPresentation=populationCountPresentation(alleleNumber),homozygotePresentation=populationCountPresentation(homozygotes),countReported=countPresentation.tone!=='missing',numberReported=numberPresentation.tone!=='missing',countAndNumber={display:countReported&&numberReported?`${countPresentation.display} of ${numberPresentation.display}`:countReported?`${countPresentation.display}; total not reported`:numberReported?`Alternate count not reported; ${numberPresentation.display} measured`:'Not reported',tone:countReported||numberReported?'neutral':'missing'},maximumDisplay={...maximumPresentation,display:maximumLabel&&maximumPresentation.tone!=='missing'?`${maximumPresentation.display} · ${maximumLabel}`:maximumPresentation.display};
  const sourceTitle=resourceTitle(primarySourceId),overallTooltip=overall?evidenceValueTooltip(overall,overall.value,{includeSource:true}):`${sourceTitle} does not report an overall allele frequency for this variant.`,maximumTooltip=maximumItem?`${evidenceValueTooltip(maximumItem,maximumItem.value,{includeSource:true})}${maximumLabel?` The highest group is ${maximumLabel}.`:''}`:`${sourceTitle} does not report an ancestry-group maximum for this variant.`,countTooltip=countReported||numberReported?`Alternate alleles: ${countPresentation.display}. Total alleles: ${numberPresentation.display}. Source: ${sourceTitle}.`:`${sourceTitle} does not report allele counts for this variant.`,homozygoteTooltip=homozygotes?evidenceValueTooltip(homozygotes,homozygotes.value,{includeSource:true}):`${sourceTitle} does not report an alternate-homozygote count for this variant.`;
  const primaryAdditional=primaryItems.filter(item=>!populationFieldKind(item)&&!populationAncestryEntry(item)),secondaryItems=items.filter(item=>sourceKey(item.sourceId)!==primaryKey),additionalItems=sortPopulationEvidence([...primaryAdditional,...secondaryItems]);
  const ancestryHtml=ancestryRows.map(row=>populationEvidenceRow(row.item,row.label,row.code===maximumCode)).join(''),additionalHtml=additionalItems.map(item=>populationEvidenceRow(item,evidenceFieldPresentation(item).label,false,sourceKey(item.sourceId)!==primaryKey)).join('');
  return`<div class="key-evidence-subgroup population-evidence" data-evidence-group="population"><div class="key-evidence-subheading fui-section-heading"><strong>Population</strong></div><div class="annotation-list fui-key-value-list population-summary-list">${populationSummaryMetric('Overall allele frequency',overallPresentation,overallTooltip)}${populationSummaryMetric('Highest group AF',maximumDisplay,maximumTooltip)}${populationSummaryMetric('Alternate alleles observed',countAndNumber,countTooltip)}${populationSummaryMetric('Alternate homozygotes',homozygotePresentation,homozygoteTooltip)}</div>${ancestryRows.length?`<details class="population-breakdown collapsible-detail fui-accordion"><summary><strong>Genetic ancestry breakdown</strong></summary><div class="annotation-list fui-key-value-list">${ancestryHtml}</div></details>`:''}${additionalItems.length?`<details class="population-breakdown collapsible-detail fui-accordion"><summary><strong>Additional frequency data (${additionalItems.length})</strong></summary><div class="annotation-list fui-key-value-list">${additionalHtml}</div></details>`:''}</div>`
}
function evidenceVectorParts(value){
  if(typeof value!=='string'||!value.includes(';'))return null;
  const all=value.split(';').map(part=>part.trim()),reported=all.filter(part=>part&&part!=='.'&&part!=='-');
  return{all,reported}
}
function evidenceVectorPresentation(item,value){
  const vector=evidenceVectorParts(value);
  if(!vector)return null;
  if(!vector.reported.length)return evidenceResult('Not reported','missing','missing','text',{summaryNote:`The ${vector.all.length} source transcript entries do not report a value.`});
  const distinct=[...new Set(vector.reported)];
  if(distinct.length===1){
    const scalar=evidenceValuePresentation(item,distinct[0]);
    return{...scalar,summaryNote:`The ${vector.reported.length} source transcript entr${vector.reported.length===1?'y reports':'ies report'} the same value.`}
  }
  const numbers=distinct.map(Number);
  if(numbers.every(Number.isFinite)){
    const minimum=Math.min(...numbers),maximum=Math.max(...numbers),format=number=>Number.isInteger(number)?String(number):plainDecimalNotation(String(Number(number.toPrecision(6)))),display=`${format(minimum)}–${format(maximum)}`,presentations=numbers.map(number=>evidenceValuePresentation(item,String(number))),calibrated=presentations.filter(presentation=>presentation.evidenceBand||presentation.predictorInterpretation);
    if(calibrated.length){
      const labels=new Set(presentations.map(presentation=>presentation.evidenceBand?.label||''));
      if(calibrated.length===presentations.length&&labels.size===1&&!labels.has('')){
        const scalar=presentations[0];
        return{...scalar,display,summaryNote:`${scalar.evidenceBand.label}. The ${vector.reported.length} source transcript entries form this range.`}
      }
      return evidenceResult(display,'neutral','neutral','text',{summaryNote:`The values from ${vector.reported.length} source transcript entries span multiple or unresolved calibrated intervals.`,predictorInterpretation:{note:'No single calibrated range was assigned because the source transcript scores do not resolve to one interval.'}})
    }
    const tones=new Set(presentations.map(presentation=>presentation.tone));
    return{...presentations[0],display,tone:tones.size===1?presentations[0].tone:'neutral',summaryNote:`The ${vector.reported.length} source transcript entries form this range.`}
  }
  const values=distinct.map(part=>evidenceValuePresentation(item,part).display),shown=values.slice(0,3),remaining=values.length-shown.length;
  return evidenceResult(`${shown.join(', ')}${remaining?` +${remaining} more`:''}`,'neutral','neutral','text',{summaryNote:`The ${vector.reported.length} source transcript entries report distinct values.`})
}
function evidenceValuePresentation(item,value=item.value){
  if(favorTranscriptSummary(item))item={...item,consequenceTerms:undefined};
  value=decodeEvidenceValue(value);
  const vectorPresentation=evidenceVectorPresentation(item,value);
  if(vectorPresentation)return vectorPresentation;
  const source=String(item.sourceId||'').toLowerCase(),field=String(item.fieldPath||''),lower=field.toLowerCase(),rawValue=Array.isArray(value)?value.map(item=>displayDetailValue(item)).join(', '):String(value??''),raw=fieldSourceIs(item,'favor-online')&&rawValue.trim()!==''&&Number.isFinite(Number(rawValue))?conciseNumericNotation(rawValue):rawValue;
  if(!raw||raw==='.'||raw==='-')return evidenceResult('Not reported','missing','missing');
  const categoricalPrediction=categoricalPredictionPresentation(item,raw);
  if(categoricalPrediction)return categoricalPrediction;
  if(/(?:^|[._])(pred|prediction)(?:$|[._])/.test(lower))return evidenceResult(readableTerm(dbnsfpPredictionValue(field,raw)).replace(/\bambiguous\b/gi,'Uncertain'),'neutral');
  if(lower==='appris')return evidenceResult(raw.replace(/^principal(\d+)$/i,'Principal isoform $1').replace(/^alternative(\d+)$/i,'Alternative isoform $1'));
  if(lower==='tsl')return evidenceResult(raw==='1'?'Level 1 (best supported)':`Level ${raw}`);
  if(lower==='aaref'||lower==='aaalt'){const amino={A:'Alanine',R:'Arginine',N:'Asparagine',D:'Aspartic acid',C:'Cysteine',E:'Glutamic acid',Q:'Glutamine',G:'Glycine',H:'Histidine',I:'Isoleucine',L:'Leucine',K:'Lysine',M:'Methionine',F:'Phenylalanine',P:'Proline',S:'Serine',T:'Threonine',W:'Tryptophan',Y:'Tyrosine',V:'Valine',X:'Unknown'};return evidenceResult(amino[raw]?`${amino[raw]} (${raw})`:raw)}
  if(lower.includes('ccre')&&lower.includes('annotation')){const labels={PLS:'Promoter-like',pELS:'Proximal enhancer-like',dELS:'Distal enhancer-like','CA-CTCF':'Accessible CTCF-bound','CA-H3K4me3':'Accessible H3K4me3','CA-TF':'Accessible transcription-factor site',CA:'Chromatin accessible','TF Only':'Transcription-factor only','CTCF-Bound':'CTCF-bound'};return evidenceResult(raw.split(/[,;]/).map(value=>labels[value.trim()]||readableTerm(value)).join(', '))}
  if(lower.includes('significance')){const display=readableTerm(raw),tone=/pathogenic/i.test(raw)&&!/conflict/i.test(raw)?'adverse':/uncertain|conflict/i.test(raw)?'caution':/benign/i.test(raw)?'reassuring':'neutral';return evidenceResult(display,tone,'source-native')}
  const number=Number(raw);
  if(Number.isFinite(number)){
    const display=plainDecimalNotation(raw),calibrated=useCalibratedEvidenceColors?calibratedPredictorInterpretation(item,number):null;
    if(calibrated?.evidenceBand){
      const band={...calibrated.evidenceBand,tone:directionTone(calibrated.evidenceBand.direction)},rank=calibrated.predictor.id==='cadd-phred'?phredRankLabel(number):'';
      return evidenceResult(display,band.tone,'calibrated','pill',{evidenceBand:band,summaryNote:[band.label,rank?`CADD PHRED rank: ${rank}`:''].filter(Boolean).join(' · '),predictorInterpretation:calibrated})
    }
    const native=nativePredictorInterpretation(item,number);
    if(native){
      const threshold=calibrationThresholdLabel(native.band);
      return evidenceResult(display,native.band.tone,'source-native','text',{nativeInterpretation:native,summaryNote:[native.band.label,threshold].filter(Boolean).join(' · '),predictorInterpretation:calibrated})
    }
    const predictor=calibratedPredictorDefinition(item);
    const nativeApplicability=predictor?.nativeInterpretation?predictorApplicability(predictor.nativeInterpretation,item):null,predictorApplicabilityResult=predictor?predictorApplicability(predictor,item):null,activeApplicability=nativeApplicability||predictorApplicabilityResult;
    if(predictor&&!activeApplicability?.applies&&activeApplicability?.reason!=='missing-context')return evidenceResult('Not applicable','neutral','not-applicable','text',{summaryNote:activeApplicability.note,predictorInterpretation:{predictor,applicable:false,note:activeApplicability.note}});
    if(predictor)return evidenceResult(display,'neutral','neutral','text',{summaryNote:calibrated?.note||'No verified directional interpretation is available for this source and field.',predictorInterpretation:calibrated||{predictor,applicable:false,note:'No verified directional interpretation is available for this source and field.'}});
    const conservationApc=conservationApcPresentation(item,number,display);
    if(conservationApc)return conservationApc;
    if(lower.includes('apc')){const rank=phredRankLabel(number);return evidenceResult(raw,'neutral','neutral','text',{summaryNote:rank?`The PHRED rank is ${rank}.`:''})}
    if(lower.includes('mappability')||lower.includes('umap')||lower.includes('bismap'))return evidenceResult(raw,number<.5?'caution':'neutral','directional');
    if(lower.includes('min_dist_tss')||lower.includes('min_dist_tse')||lower.includes('min_dist_tes'))return evidenceResult(number>=1000?`${(number/1000).toLocaleString(undefined,{maximumFractionDigits:2})} kb`:`${number.toLocaleString()} bp`);
    if(alleleFrequencyField(source,field))return evidenceResult(display,alleleFrequencyTone(raw),'source-native','text',{summaryNote:alleleFrequencySummary(raw)});
  }
  return evidenceResult(readableTerm(raw));
}

function evidenceValueTooltip(item,value=item.value,{includeSource=false,resolution=null,interpretation=null}={}){
  const presentation=evidenceFieldPresentation(item),interpreted=interpretation||evidenceValuePresentation(item,value),raw=displayDetailValue(value),plainRaw=fieldSourceIs(item,'favor-online')&&Number.isFinite(Number(raw))?conciseNumericNotation(raw):plainDecimalNotation(raw),categoryDefinition=categoricalPredictionDefinition(item),transcriptSummary=favorTranscriptSummary(item),parts=[],add=part=>{const text=String(part||'').trim();if(text)parts.push(/[.!?]$/.test(text)?text:`${text}.`)};
  if(interpreted.tier==='missing')add(`No value is reported for ${presentation.label}`);
  else if(categoryDefinition)add(`Prediction: ${interpreted.display}`);
  else if(interpreted.evidenceBand){
    const threshold=calibrationThresholdLabel(interpreted.evidenceBand);
    add(`Score: ${plainRaw}. Calibrated interpretation: ${conciseEvidenceBandLabel(interpreted.evidenceBand.label)}${threshold?` (${threshold})`:''}`)
  }else if(interpreted.nativeInterpretation){
    const {policy,band}=interpreted.nativeInterpretation,threshold=calibrationThresholdLabel(band);
    add(`Score: ${plainRaw}. Interpretation: ${band.label}${threshold?` (${threshold})`:''}`)
  }else if(evidenceVectorParts(raw))add(`The source values are ${interpreted.display}`);
  else if(alleleFrequencyField(item.sourceId,item.fieldPath)){
    add(`Allele frequency: ${interpreted.display}`);
    add(interpreted.summaryNote||'Frequency alone does not determine pathogenicity')
  }
  else{
    add(`Value: ${interpreted.display}`);
    const guide=interpreted.summaryNote||presentation.readingGuide||presentation.baseDescription;
    add(guide)
  }
  if(resolution?.kind==='ambiguous')add('No value matches the selected transcript. The range includes all reported transcripts');
  else if(resolution?.kind==='invalid_vector')add('Transcript matching failed. The range includes all reported values');
  else if(resolution?.kind==='source_selected'||favorSourceSelectedCoding(item))add('FAVOR selected this coding record. AnnoCAT did not match it to the selected transcript');
  else if(item.sourceCardinality==='alignedVector')add('This value matches the selected transcript');
  else if(item.sourceCardinality==='recordScalar')add('dbNSFP reports one value for this variant. The value does not change with the selected transcript');
  if(categoryDefinition?.note&&!transcriptSummary)add(categoryDefinition.note);
  if(transcriptSummary)add(transcriptSummary.note);
  if(includeSource)add(`Source: ${resourceTitle(item.sourceId)}`);
  return parts.filter(Boolean).join(' ')
}
function evidencePresentation(item,value=item.value,options={}){
  const interpreted=evidenceValuePresentation(item,value);
  return{...interpreted,tooltip:evidenceValueTooltip(item,value,{...options,interpretation:interpreted})}
}

function selectedTranscriptEvidence(items,transcriptId){
  const normalizeTranscript=value=>String(value||'').trim().split('.')[0],target=normalizeTranscript(transcriptId);
  return items.flatMap(item=>{
    if(item.sourceCardinality==='recordScalar')return[{...item,scopeLabel:'Variant'}];
    let group=resultAlignmentGroups.find(group=>group.sourceId===item.sourceId&&group.scope===item.scope&&group.fields?.includes(item.fieldPath));
    if(!group&&String(item.sourceId).toLowerCase()==='dbnsfp'&&!dbnsfpVariantLevelField(item.fieldPath))group={keyField:'Ensembl_transcriptid',separator:';'};
    if(!group)return[{...item,scopeLabel:item.scope==='transcript'?'Transcript':'Variant'}];
    const transcriptField=items.find(candidate=>candidate.sourceId===item.sourceId&&candidate.scope===item.scope&&candidate.fieldPath===group.keyField&&typeof candidate.value==='string'),separator=group.separator||';',transcripts=transcriptField?transcriptField.value.split(separator).map(normalizeTranscript):[],matches=transcripts.map((value,index)=>value===target?index:-1).filter(index=>index>=0),index=matches.length===1?matches[0]:-1;
    if(index<0)return[{...item,scopeLabel:'All reported transcripts',unmatchedTranscript:true}];
    if(typeof item.value==='string'&&item.value.includes(';')){
      const values=item.value.split(separator);
      if(values.length===transcripts.length)return[{...item,value:values[index],scopeLabel:'Transcript'}];
      return[{...item,scopeLabel:'All reported transcripts',unmatchedTranscript:true}]
    }
    return[{...item,scopeLabel:'Transcript'}];
  });
}
function evidenceForSelectedConsequence(items,consequenceId,transcriptId,isTranscript=true){
  const scoped=items.filter(item=>item.scope==='allele'||!consequenceId||!item.consequenceId||item.consequenceId===consequenceId);
  return isTranscript?selectedTranscriptEvidence(scoped,transcriptId):scoped
}

const evidenceDomainTitles={key:'Key evidence',clinical:'Clinical evidence',population:'Population frequency',prediction:'Prediction scores',splicing:'Splicing',conservation:'Conservation',functional:'Functional annotation summaries',regulatory:'Regulatory and noncoding',gene:'Gene relationships',technical:'Technical reliability',regional:'Regional context',other:'Other evidence'};
const evidenceDomainOrder=['clinical','population','prediction','splicing','conservation','functional','regulatory','gene','technical','regional','other'];
function evidenceDomain(item){
  const source=String(item?.sourceId||'').toLowerCase(),field=String(item?.fieldPath||'').toLowerCase(),text=`${source} ${field}`;
  if(source==='hpo'||source==='gene-profile')return'phenotype';
  if(alleleFrequencyField(source,field))return'population';
  if(/clinvar|clingen|clnsig|clinical|review.?status|condition|phenotype|disease/.test(text))return'clinical';
  if(/gnomad|topmed|bravo|1000.?genomes|population|allele.?frequency|(^|[._])af([._]|$)|faf|nhomalt|allele.?count|allele.?number/.test(text))return'population';
  if(/splice|(^|[._])ds_(ag|al|dg|dl)([._]|$)/.test(text))return'splicing';
  if(/phylop|phastcons|gerp|conservation|conserved/.test(text))return'conservation';
  if(source.includes('favor')&&/^apc(epigenetics|proteinfunction)$/.test(field))return'functional';
  if(/mappability|umap|bismap|low.?complexity|repeat.?mask|segmental.?dup/.test(text))return'technical';
  if(/ccre|enhancer|promoter|remap|transcription.?factor|chromhmm|chromatin|epigen|histone|eqtl|sqtl|caqtl|microrna|mirna|cage|dnase|atac/.test(text))return'regulatory';
  if(/target.?gene|genehancer|nearest.?gene|min_dist_(tss|tse|tes)|distance.*(gene|coding|tss|tes)|proximity/.test(text))return'gene';
  if(/mutation.?density|variant.?density|nucleotide.?diversity|nucdiv|recombination|mutation.?rate|genomic.?context/.test(text))return'regional';
  if(/sift|polyphen|alphamissense|primateai|revel|meta.?svm|mutationtaster|mutationassessor|bayesdel|vest4|mutpred|varity|esm1b|(^|[._])mpc|protein.?function|amino.?acid/.test(text))return'prediction';
  if(/cadd|fathmm|linsight|jarvis|remm|ncboost|macie|gnocchi|ncer|gpn|deleterious|pathogenicity.?score/.test(text))return'prediction';
  return'other';
}
function mirroredEvidenceKey(item){
  const presentation=evidenceFieldPresentation(item),decoded=decodeEvidenceValue(item.value),raw=Array.isArray(decoded)?decoded.join('|'):String(decoded??'').trim(),number=Number(raw),value=raw&&Number.isFinite(number)?String(number):raw.toLowerCase();
  return`${presentation.label.toLowerCase()}\u001f${value}`
}
function deduplicateMirroredEvidence(items){
  const result=[],keys=new Map();
  items.forEach(item=>{
    const key=mirroredEvidenceKey(item),existingIndex=keys.get(key);
    if(existingIndex===undefined){keys.set(key,result.length);result.push(item);return}
    const existing=result[existingIndex],existingFavor=fieldSourceIs(existing,'favor-online'),itemFavor=fieldSourceIs(item,'favor-online');
    if(existingFavor&&!itemFavor)result[existingIndex]=item;
    else if(!existingFavor&&!itemFavor)result.push(item)
  });
  return result
}
function predictorFamily(item){
  const predictor=calibratedPredictorDefinition(item);
  if(predictor)return predictor.id;
  const field=String(categoricalPredictionDefinition(item)?.fieldName||'').toLowerCase().replace(/[^a-z0-9]/g,'');
  if(field.startsWith('sift'))return'sift';
  if(field.startsWith('polyphen'))return'polyphen';
  if(field.startsWith('metasvm'))return'metasvm';
  return field
}
function preferLocalPredictorEvidence(items){
  const localFamilies=new Set(items.filter(item=>!fieldSourceIs(item,'favor-online')).map(predictorFamily).filter(Boolean));
  return items.filter(item=>{const family=predictorFamily(item);return!fieldSourceIs(item,'favor-online')||!family||!localFamilies.has(family)})
}
function predictionSummaryFamily(item){
  const category=categoricalPredictionDefinition(item);
  if(!category||category.excludeFromSummary||String(category.fieldName).toLowerCase()==='aloft_pred')return null;
  return{
    key:String(category.fieldName).toLowerCase().replace(/_pred$/,''),
    label:readableTerm(String(category.fieldName).replace(/_pred$/i,''))
  }
}
function predictionSummaryBar(items){
  items=items.filter(item=>!favorTranscriptSummary(item)&&!favorSourceSelectedCoding(item));
  const families=new Map(),primaryCalibrated=new Map();
  items.forEach(item=>{
    const presentation=evidenceValuePresentation(item),family=predictionSummaryFamily(item);
    if(family&&presentation.categoryInterpretation&&!families.has(family.key))families.set(family.key,{...family,presentation,item});
    const band=presentation.evidenceBand;
    if(band?.role==='primary'&&!primaryCalibrated.has(band.evidenceGroup))primaryCalibrated.set(band.evidenceGroup,{presentation,item,label:band.predictorLabel})
  });
  const predictions=[...families.values()],primary=[...primaryCalibrated.values()];
  if(!predictions.length&&!primary.length)return'<div class="prediction-summary"><dt>Prediction summary</dt><dd class="tone-missing">Not reported</dd></div>';
  const groups=[
    {key:'adverse',label:'damaging',title:'Damaging'},
    {key:'caution',label:'uncertain',title:'Uncertain'},
    {key:'reassuring',label:'benign',title:'Benign or tolerated'},
    {key:'neutral',label:'no direction',title:'No clear direction'}
  ].map(group=>({...group,items:predictions.filter(item=>item.presentation.tone===group.key)})).filter(group=>group.items.length);
  const details=groups.flatMap(group=>group.items.map(item=>`${item.label}: ${item.presentation.display}`)).join(' · '),label=groups.map(group=>`${group.items.length} ${group.label}`).join(', '),primaryHtml=primary.map(({presentation,item,label})=>`<span class="prediction-summary-primary ${evidenceClass(presentation)}" title="${escapeHtml(evidenceValueTooltip(item,item.value,{includeSource:true,interpretation:presentation}))}">${escapeHtml(`${label} ${presentation.display}`)}</span>`).join(''),barHtml=predictions.length?`<span class="prediction-summary-bar" role="img" aria-label="${escapeHtml(`${predictions.length} direct categorical predictors: ${label}`)}" title="${escapeHtml(`${details}. Counts include only categorical predictions reported by the source.`)}">${groups.map(group=>`<span class="prediction-summary-segment tone-${group.key}" style="flex-grow:${group.items.length}" title="${escapeHtml(`${group.title}: ${group.items.map(item=>item.label).join(', ')}`)}"><b>${group.items.length}</b></span>`).join('')}</span>`:'';
  return`<div class="prediction-summary"><dt>Prediction summary</dt><dd class="prediction-summary-content">${primaryHtml}${barHtml}</dd></div>`
}
function numericEvidenceValues(value){
  const decoded=decodeEvidenceValue(value),values=Array.isArray(decoded)?decoded:typeof decoded==='string'?decoded.split(';'):[decoded];
  return values.map(value=>Number(String(value).trim())).filter(value=>Number.isFinite(value))
}
function spliceAiMaximumDeltaItem(items){
  if(items.some(item=>fieldSourceIs(item,'spliceai')&&evidenceFieldLeaf(item).toLowerCase().replace(/[^a-z0-9]/g,'')==='maxdeltascore'))return null;
  const components=items.filter(item=>fieldSourceIs(item,'spliceai')&&['dsag','dsal','dsdg','dsdl'].includes(evidenceFieldLeaf(item).toLowerCase().replace(/[^a-z0-9]/g,''))),scores=components.flatMap(item=>numericEvidenceValues(item.value)).filter(value=>value>=0&&value<=1);
  if(!scores.length)return null;
  const source=components[0];
  return{...source,fieldPath:'maxDeltaScore',value:String(Math.max(...scores)),scope:'allele',scopeLabel:'Variant-level',synthetic:true}
}
function evidenceScopeLabel(item){if(fieldSourceIs(item,'favor-online')&&evidenceFieldLeaf(item).toLowerCase().startsWith('coding'))return'FAVOR selected coding record';if(item.scopeLabel)return item.scopeLabel==='Transcript'?'Selected transcript':item.scopeLabel;return item.scope==='transcript'?'Selected transcript':'Variant-level'}
function clinicalListValues(item,value){
  const field=String(item.fieldPath||'').toLowerCase();
  if(!/condition|phenotype|disease/.test(field))return[];
  const decoded=decodeEvidenceValue(value),values=Array.isArray(decoded)?decoded:typeof decoded==='string'&&decoded.includes('|')?decoded.split('|'):[];
  return[...new Set(values.map(item=>readableTerm(displayDetailValue(item))).filter(item=>item&&item!=='.'&&item!=='-'))]
}
function annotationRow(item,value=item.value){
  const presentation=evidenceFieldPresentation(item),resolution=(item.resolution||item.unmatchedTranscript)?{kind:item.resolution?.kind||'ambiguous'}:null,interpreted=evidencePresentation(item,value,{includeSource:true,resolution}),tooltip=interpreted.tooltip,listValues=clinicalListValues(item,value),rendered=listValues.length>1?`<ul class="annotation-value-list">${listValues.map(item=>`<li>${escapeHtml(item)}</li>`).join('')}</ul>`:`<b class="${evidenceClass(interpreted)}">${escapeHtml(interpreted.display)}</b>`;
  return`<div class="annotation-row fui-key-value-row tone-${interpreted.tone}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(presentation.label)}</strong></span>${rendered}</div>`
}
function detailEvidenceSubgroup(title,items,empty='Not reported',options={}){if(options.kind==='population')return populationEvidenceSubgroup(items,options.preferredSourceId,empty);return`<div class="key-evidence-subgroup" data-evidence-group="${escapeHtml(title.toLowerCase())}"><div class="key-evidence-subheading fui-section-heading"><strong>${escapeHtml(title)}</strong></div><div class="annotation-list fui-key-value-list">${items.length?items.map(item=>annotationRow(item)).join(''):`<div class="key-evidence-empty">${escapeHtml(empty)}</div>`}</div></div>`}
function groupedEvidenceSection(title,groups,{open=false,className='',extra='',sectionKey=''}={}){
  const expanded=sectionKey?variantDetailOpenSections.has(sectionKey):open,stateAttribute=sectionKey?` data-variant-detail-section="${escapeHtml(sectionKey)}"`:'';
  return`<section class="detail-section annotation-section evidence-domain-section fui-accordion-stack ${escapeHtml(className)}"><details class="evidence-domain collapsible-detail fui-accordion"${stateAttribute} ${expanded?'open':''}><summary><strong>${escapeHtml(title)}</strong></summary><div class="key-evidence-subgroups">${groups.map(([groupTitle,items,empty,options])=>detailEvidenceSubgroup(groupTitle,items,empty,options)).join('')}${extra}</div></details></section>`
}
function phenotypeEvidenceExtra(items){
  const item=items.find(candidate=>String(candidate.fieldPath||'')==='phenotypeEvidenceDetails');
  if(!item)return'';
  const details=decodeEvidenceValue(item.value);
  if(!details||typeof details!=='object')return'';
  const matches=Array.isArray(details.bestPhenotypeMatch?.matches)?details.bestPhenotypeMatch.matches:[],links=Array.isArray(details.conditionLinks)?details.conditionLinks:[],sections=[];
  if(matches.length){
    sections.push(`<div class="key-evidence-subgroup"><div class="key-evidence-subheading fui-section-heading"><strong>Feature matches</strong></div><div class="annotation-list fui-key-value-list">${matches.map(match=>{const query=match.query||{},disease=match.diseaseTerm||{},relation=match.direct?'Exact feature':'Related feature',score=Number(match.similarity);return`<div class="annotation-row fui-key-value-row fui-key-value-row--stacked"><span class="annotation-field fui-key-value-cell"><strong>${escapeHtml(query.label||query.id||'Observed feature')}</strong><small>${escapeHtml(query.id||'')}</small></span><span class="annotation-value fui-key-value-cell"><b>${escapeHtml(disease.label||disease.id||'Not reported')}</b><small>${escapeHtml([disease.id,relation,Number.isFinite(score)?`Score ${score}`:''].filter(Boolean).join(' · '))}</small></span></div>`}).join('')}</div></div>`)
  }
  if(links.length){
    sections.push(`<div class="key-evidence-subgroup"><div class="key-evidence-subheading fui-section-heading"><strong>Condition associations</strong></div><div class="annotation-list fui-key-value-list">${links.map(link=>{const source=String(link.associationSource||''),sourceLabel=/mim2gene/i.test(source)?'NCBI mim2gene/MedGen':source.replace(/^https?:\/\/|^ftp:\/\//i,'').split('/')[0],matched=[link.matchedCondition,link.matchedConditionId].filter(Boolean).join(' · '),provenance=[link.associationType,sourceLabel].filter(Boolean).join(' · ');return`<div class="annotation-row fui-key-value-row fui-key-value-row--stacked"><span class="annotation-field fui-key-value-cell"><strong>${escapeHtml(link.selectedCondition||link.selectedConditionId||'Selected condition')}</strong><small>${escapeHtml(link.selectedConditionId||'')}</small></span><span class="annotation-value fui-key-value-cell"><b>${escapeHtml(link.relation||'Not reported')}</b>${matched?`<small>${escapeHtml(matched)}</small>`:''}${provenance?`<small>${escapeHtml(provenance)}</small>`:''}</span></div>`}).join('')}</div></div>`)
  }
  return sections.join('')
}
function geneMatchEvidenceExtra(items){
  const item=items.find(candidate=>String(candidate.fieldPath||'')==='geneMatchDetails');
  if(!item)return'';
  const matches=decodeEvidenceValue(item.value);
  if(!Array.isArray(matches)||!matches.length)return'';
  const rows=matches.map(match=>{
    const itemId=String(match.selectedItemId||''),pathway=itemId.startsWith('R-HSA-'),label=match.selectedItem||itemId||'Selected item',title=pathway?`<a href="${escapeHtml(`https://reactome.org/content/detail/${encodeURIComponent(itemId)}`)}" target="_blank" rel="noopener noreferrer">${escapeHtml(label)} ↗</a>`:`<strong>${escapeHtml(label)}</strong>`;
    return`<div class="annotation-row fui-key-value-row fui-key-value-row--stacked"><span class="annotation-field fui-key-value-cell">${title}<small>${escapeHtml([itemId,match.itemType].filter(Boolean).join(' · '))}</small></span><span class="annotation-value fui-key-value-cell"><b>${escapeHtml(match.geneSymbol||match.geneId||'Gene not reported')}</b><small>${escapeHtml([match.geneId,match.relation].filter(Boolean).join(' · '))}</small></span></div>`
  }).join('');
  return`<div class="key-evidence-subgroup"><div class="key-evidence-subheading fui-section-heading"><strong>Selected matches</strong></div><div class="annotation-list fui-key-value-list">${rows}</div></div>`
}
function rememberVariantDetailSectionState(){
  $('#variant-detail-body')?.querySelectorAll('details[data-variant-detail-section]').forEach(section=>section.open?variantDetailOpenSections.add(section.dataset.variantDetailSection):variantDetailOpenSections.delete(section.dataset.variantDetailSection))
}
function bindVariantDetailSectionState(){
  $('#variant-detail-body')?.querySelectorAll('details[data-variant-detail-section]').forEach(section=>section.addEventListener('toggle',()=>section.open?variantDetailOpenSections.add(section.dataset.variantDetailSection):variantDetailOpenSections.delete(section.dataset.variantDetailSection)))
}

function transcriptMetadataFact(metadata,fieldPath,labelOverride=''){
  const item=metadata.find(entry=>String(entry.fieldPath||'').toLowerCase()===fieldPath.toLowerCase());
  if(!item)return null;
  const presentation=evidenceFieldPresentation(item),interpreted=evidencePresentation(item,item.value,{includeSource:true});
  return[labelOverride||presentation.label,interpreted.display,interpreted.tooltip,interpreted.tone]
}
function transcriptFactGroup(title,facts){
  const rendered=facts.filter(Boolean).map(([label,value,tooltip='',tone='neutral'])=>`<div class="fui-key-value-row" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="tone-${escapeHtml(tone)}">${escapeHtml(displayDetailValue(value))}</dd></div>`).join('');
  return`<section class="transcript-fact-group"><div class="transcript-fact-heading fui-section-heading"><strong>${escapeHtml(title)}</strong></div><dl class="fui-description-grid">${rendered}</dl></section>`
}
function compactHgvs(value){
  const text=String(value||''),description=text.slice(text.lastIndexOf(':')+1);
  return/^(?:c|n|r|m|p|g)\./i.test(description)?description:text
}
function consequenceContextKey(item,index=0){
  const type=String(consequenceValue(item,'feature_type')||'transcript'),id=consequenceValue(item,'feature_id','transcript_id','Feature','regulatory_feature_id','motif_feature_id')||item._annocatConsequenceId||index;
  return`${type}:${id}`
}
function preferredConsequence(items,representativeTranscriptId=''){
  const transcript=value=>String(value||'').trim(),normalizeTranscript=value=>transcript(value).split('.')[0],representativeTranscript=transcript(representativeTranscriptId),representative=normalizeTranscript(representativeTranscript);
  if(representativeTranscript){
    const exact=items.find(item=>transcript(consequenceValue(item,'transcript_id','Feature'))===representativeTranscript);
    if(exact)return exact;
    const stableMatches=items.filter(item=>normalizeTranscript(consequenceValue(item,'transcript_id','Feature'))===representative);
    if(stableMatches.length===1)return stableMatches[0]
  }
  const ranked=items.map((item,index)=>{
    const type=String(consequenceValue(item,'feature_type')||'transcript').toLowerCase(),mane=consequenceValue(item,'mane_select','MANE_SELECT','MANE'),manePlus=consequenceValue(item,'mane_plus_clinical','MANE_PLUS_CLINICAL'),canonical=consequenceValue(item,'canonical','CANONICAL'),isCanonical=canonical===true||canonical===1||['YES','Y','TRUE','1'].includes(String(canonical).toUpperCase()),rank=type==='transcript'?(mane?0:manePlus?1:isCanonical?2:3):4;
    return{item,index,rank}
  });
  return ranked.sort((left,right)=>left.rank-right.rank||left.index-right.index)[0]?.item||null
}
function transcriptDetail(item,metadata=[]){
  const terms=consequenceValue(item,'consequence_terms','Consequence'),primaryTerm=primaryConsequence(terms),secondaryTerms=additionalConsequences(terms),impact=readableTerm(consequenceValue(item,'impact','IMPACT')),canonical=consequenceValue(item,'canonical','CANONICAL'),canonicalReported=canonical!==null&&canonical!==undefined&&canonical!=='',isCanonical=canonical===true||canonical===1||String(canonical).toUpperCase()==='YES',mane=consequenceValue(item,'mane_select','MANE_SELECT','MANE'),manePlus=consequenceValue(item,'mane_plus_clinical','MANE_PLUS_CLINICAL'),exon=consequenceValue(item,'exon','EXON'),intron=consequenceValue(item,'intron','INTRON'),hgvsc=consequenceValue(item,'hgvsc','HGVSc'),hgvsp=consequenceValue(item,'hgvsp','HGVSp');
  const aminoRef=transcriptMetadataFact(metadata,'aaref'),aminoAlt=transcriptMetadataFact(metadata,'aaalt'),aminoPosition=transcriptMetadataFact(metadata,'aapos'),aminoName=fact=>String(fact?.[1]||'').replace(/\s+\([A-Z*]\)$/,'');
  const proteinChange=aminoRef||aminoAlt||aminoPosition?[aminoRef&&aminoAlt?`${aminoName(aminoRef)} > ${aminoName(aminoAlt)}`:aminoName(aminoRef)||aminoName(aminoAlt),aminoPosition?`at position ${aminoPosition[1]}`:''].filter(Boolean).join(' '):'';
  const locationFacts=[exon?['Exon',exon,'Affected exon and total exon count when reported.']:null,intron?['Intron',intron,'Affected intron and total intron count when reported.']:null];
  const primaryFacts=[
    ['Gene',consequenceValue(item,'gene_symbol','SYMBOL','gene_id','Gene','transcript_id','Feature'),'Gene symbol associated with the selected transcript, or its stable Ensembl identifier when no symbol is assigned.'],
    ['Primary consequence',readableTerm(primaryTerm),'First Sequence Ontology consequence reported by VEP for the selected transcript.'],
    secondaryTerms.length?['Additional consequences',secondaryTerms.map(readableTerm).join(', '),'Additional Sequence Ontology terms reported for the same transcript.']:null,
    ['HGVSc',compactHgvs(hgvsc),`Full HGVS coding description: ${displayDetailValue(hgvsc)}`],
    ['HGVSp',compactHgvs(hgvsp),`Full HGVS protein description: ${displayDetailValue(hgvsp)}`],
    proteinChange?['Protein change',proteinChange,[aminoRef?.[2],aminoAlt?.[2],aminoPosition?.[2]].filter(Boolean).join(' · ')]:null,
    ...locationFacts,
    ['Biotype',readableTerm(consequenceValue(item,'biotype','BIOTYPE')),'Transcript biotype reported by VEP.'],
    ['Impact',impact,'VEP impact category for the selected transcript.',variantFactTone('Impact',impact)]
  ];
  const qualityFacts=[
    ['MANE',mane||'Not designated','MANE Select designation for the selected transcript.',mane?'informative':'missing'],
    manePlus?['MANE Plus Clinical',manePlus,'Supplemental MANE transcript for clinically relevant content not represented by MANE Select.','informative']:null,
    ['Canonical',canonicalReported?(isCanonical?'Yes':'No'):'Not reported','Whether Ensembl marks the selected transcript as canonical.',canonicalReported?(isCanonical?'reassuring':'neutral'):'missing'],
    transcriptMetadataFact(metadata,'APPRIS'),
    transcriptMetadataFact(metadata,'GENCODE_basic'),
    transcriptMetadataFact(metadata,'TSL','Transcript support')
  ];
  const identifierFacts=[
    ['Transcript',consequenceValue(item,'transcript_id','Feature'),'Stable Ensembl transcript identifier.'],
    ['Gene ID',consequenceValue(item,'gene_id','Gene'),'Stable Ensembl gene identifier.'],
    ['Protein',consequenceValue(item,'protein_id','ENSP'),'Stable Ensembl protein identifier.'],
    transcriptMetadataFact(metadata,'Uniprot_acc','UniProt')
  ];
  return`<div class="transcript-fact-groups">${transcriptFactGroup('Selected transcript effect',primaryFacts)}${transcriptFactGroup('Transcript selection & support',qualityFacts)}${transcriptFactGroup('Reference identifiers',identifierFacts)}</div>`
}
function nonTranscriptDetail(item){
  const rawType=String(consequenceValue(item,'feature_type')||'unresolved'),featureType=readableTerm(rawType),terms=consequenceValue(item,'consequence_terms','Consequence'),primaryTerm=primaryConsequence(terms),secondaryTerms=additionalConsequences(terms),impact=readableTerm(consequenceValue(item,'impact','IMPACT'));
  const effectFacts=[
    ['Feature type',featureType,'VEP consequence feature class.'],
    ['Gene',consequenceValue(item,'gene_symbol','SYMBOL','gene_id','Gene'),'Gene symbol associated with this feature, or its stable Ensembl gene identifier when no symbol is assigned.'],
    ['Primary consequence',readableTerm(primaryTerm),'First Sequence Ontology consequence reported by VEP for the selected feature.'],
    secondaryTerms.length?['Additional consequences',secondaryTerms.map(readableTerm).join(', '),'Additional Sequence Ontology terms reported for the same feature.']:null,
    ['Impact',impact,'VEP impact category for the selected feature.',variantFactTone('Impact',impact)],
    ['Distance',consequenceValue(item,'distance','DISTANCE'),'Distance in bases when VEP reports a nearby feature relationship.'],
    ['Biotype',readableTerm(consequenceValue(item,'biotype','BIOTYPE')),'Feature biotype when VEP reports one.']
  ];
  const identifierFacts=[
    ['Feature',consequenceValue(item,'feature_id','regulatory_feature_id','motif_feature_id','Feature'),'Stable feature identifier when available.'],
    ['Gene ID',consequenceValue(item,'gene_id','Gene'),'Stable Ensembl gene identifier when available.']
  ];
  return`<div class="transcript-fact-groups">${transcriptFactGroup('Selected feature effect',effectFacts)}${transcriptFactGroup('Reference identifiers',identifierFacts)}</div>`
}

function sampleCallRelationLabel(call){
  const labels={reference:'Reference',otherAlternate:'Other alternate',heterozygous:'Heterozygous',homozygousAlternate:'Homozygous alternate',haploidAlternate:'Haploid alternate',partiallyCalled:'Partially called',notCalled:'Not called',unavailable:'Not available',invalid:'Invalid genotype'};
  if(call.genotypeRelation==='mixedAlternate')return`${Number(call.selectedAltCopyCount).toLocaleString()} of ${Number(call.ploidy).toLocaleString()} copies`;
  return labels[call.genotypeRelation]||'Not available'
}
function sampleCallPhaseLabel(call){return{phased:'Phased',unphased:'Unphased',partiallyPhased:'Partially phased',haploid:'Haploid',unknown:'Not available'}[call.phase]||'Not available'}
function sampleOverview(variant){
  const missing='Not available';
  const calls=variant.sampleCalls||[];
  if(!calls.length)return{zygosity:missing,genotype:missing,depth:missing,quality:missing,referenceReads:missing,alternateReads:missing,alleleBalance:missing,phase:missing,sampleName:'',allelePresence:'unknown'};
  if(calls.length>1)return{zygosity:`${calls.length} sample calls`,genotype:'Multiple',depth:missing,quality:missing,referenceReads:missing,alternateReads:missing,alleleBalance:missing,phase:'Multiple',sampleName:'',allelePresence:'unknown'};
  const call=calls[0],fraction=Number(call.selectedAltFraction);
  return{zygosity:sampleCallRelationLabel(call),genotype:call.genotype||missing,depth:call.depth===null||call.depth===undefined?missing:`${call.depth}×`,quality:call.genotypeQuality===null||call.genotypeQuality===undefined?missing:call.genotypeQuality,referenceReads:call.referenceDepth===null||call.referenceDepth===undefined?missing:call.referenceDepth,alternateReads:call.selectedAltDepth===null||call.selectedAltDepth===undefined?missing:call.selectedAltDepth,alleleBalance:Number.isFinite(fraction)?`${(fraction*100).toLocaleString(undefined,{maximumFractionDigits:1})}%`:missing,phase:sampleCallPhaseLabel(call),sampleName:call.sampleName||'',allelePresence:call.allelePresence||'unknown'};
}
function variantFactTone(label,value){
  const text=String(value??'').trim(),lower=text.toLowerCase();
  if(!text||text==='—'||/not available|not reported|not called/.test(lower))return'missing';
  const qualityPolicy=evidenceCalibrations.displayPolicies?.quality||{},qualityContext=qualityPolicy.contextTone||'neutral';
  if(label==='Impact')return/high/.test(lower)?'adverse':/moderate/.test(lower)?'caution':'neutral';
  if(label==='Gene'||label==='Consequence')return'neutral';
  if(label==='Zygosity'||label==='Genotype / phase')return'neutral';
  if(['Depth','GQ','QUAL','Allelic support','Allele balance'].includes(label))return Number.isFinite(Number(text.replace(/[%×]/g,'')))?qualityContext:'neutral';
  if(label==='Overall AF'||label==='Group-max AF')return alleleFrequencyTone(text);
  if(label==='VCF FILTER')return/^pass$/i.test(text)?qualityPolicy.passTone||'reassuring':text==='.'?qualityPolicy.missingTone||'missing':qualityPolicy.failureTone||'adverse';
  return'neutral'
}
function variantSummaryCell(label,value,tone,tooltip,presentation='text'){
  const resolvedTone=tone||variantFactTone(label,value);
  return`<div class="fui-description-grid__item" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="${evidenceClass({tone:resolvedTone,presentation})}">${escapeHtml(displayDetailValue(value))}</dd></div>`
}
function variantSummaryPartsCell(label,parts,tooltip){
  const rendered=parts.map(({value,tone='neutral',separator=''})=>`${separator?`<span class="variant-summary-separator">${escapeHtml(separator)}</span>`:''}<span class="tone-${escapeHtml(tone)}">${escapeHtml(displayDetailValue(value))}</span>`).join('');
  return`<div class="fui-description-grid__item" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="variant-summary-parts">${rendered}</dd></div>`
}
function populationFrequencySummary(overall,groupMaximum,overallTooltip,groupMaximumTooltip){
  return`<div class="population-af-summary fui-description-grid__item"><dt>Population AF</dt><dd><span class="population-af-value tone-${variantFactTone('Overall AF',overall)}" title="${escapeHtml(overallTooltip)}">${escapeHtml(displayDetailValue(overall))}</span><small>Group max: <span class="population-af-group-value tone-${variantFactTone('Group-max AF',groupMaximum)}" title="${escapeHtml(groupMaximumTooltip)}">${escapeHtml(displayDetailValue(groupMaximum))}</span></small></dd></div>`
}
function variantSummaryRow(cells){return`<div class="detail-summary-row fui-description-grid__row" style="--detail-summary-columns:${cells.length}">${cells.join('')}</div>`}

function renderVariantDetail(row,detail){
  rememberVariantDetailSectionState();
  const consequences=detail.consequences||[],evidence=detail.evidence||[],variant=detail.variant||{},unique=[...new Map(consequences.map((item,index)=>[consequenceContextKey(item,index),item])).values()],preferred=preferredConsequence(unique,variant.transcriptId||row.transcriptId),stored=detailConsequenceSelections.get(detail.alleleId||row.alleleId),selected=unique.find((item,index)=>consequenceContextKey(item,index)===stored)||preferred||unique[0]||{},selectedContext=unique.length?consequenceContextKey(selected,unique.indexOf(selected)):'',selectedFeatureType=String(consequenceValue(selected,'feature_type')||'transcript').toLowerCase(),isTranscript=selectedFeatureType==='transcript',transcriptId=isTranscript?String(consequenceValue(selected,'transcript_id','Feature')||''):'',geneSymbol=consequenceValue(selected,'gene_symbol','SYMBOL')||variant.geneSymbol||row.geneSymbol||'',gene=geneSymbol||consequenceValue(selected,'gene_id','Gene')||variant.geneId||row.geneId||transcriptId||variant.transcriptId||row.transcriptId||'',links=usefulVariantLinks(row,geneSymbol,selected,variant);
  const selectedTerms=consequenceValue(selected,'consequence_terms','Consequence'),selectedConsequenceId=selected._annocatConsequenceId,transcriptMetadataFields=new Set(['APPRIS','GENCODE_basic','TSL','Uniprot_acc','aaref','aaalt','aapos']),alignedEvidence=evidenceForSelectedConsequence(evidence,selectedConsequenceId,transcriptId,isTranscript),metadata=isTranscript?alignedEvidence.filter(item=>String(item.sourceId).toLowerCase()==='dbnsfp'&&transcriptMetadataFields.has(item.fieldPath)):[],combined=alignedEvidence.filter(item=>!dbnsfpTranscriptMetadata.has(item.fieldPath)).map(item=>({...item,consequenceTerms:selectedTerms})),frequency=primaryAlleleFrequency(combined),frequencySourceItems=frequency?combined.filter(item=>String(item.sourceId||'').toLowerCase()===String(frequency.sourceId||'').toLowerCase()):combined,groupMaximum=groupMaximumAlleleFrequency(frequencySourceItems),frequencyDisplay=frequency?evidenceValuePresentation(frequency).display:reportHasFrequencyFields()?'Not reported':'Not available',groupMaximumDisplay=groupMaximum?evidenceValuePresentation(groupMaximum).display:reportHasFrequencyFields()?'Not reported':'Not available',domainGroups=combined.reduce((groups,item)=>{const domain=evidenceDomain(item);(groups[domain]??=[]).push(item);return groups},{}),sampleFacts=sampleOverview(variant);
  const conservationCandidates=deduplicateMirroredEvidence(domainGroups.conservation||[]),conservationSummaryItem=preferredConservationSummary(evidence),conservationSummaryPresentation=conservationSummaryItem?evidencePresentation(conservationSummaryItem,conservationSummaryItem.value,{includeSource:true}):{display:'Not reported',tone:'missing',tooltip:'The result does not contain a phyloP or FAVOR conservation aPC score for this position.'},conservationSummaryTooltip=conservationSummaryPresentation.tooltip;
  const clinvarItem=(domainGroups.clinical||[]).find(item=>/(^|[._])(significance|clnsig)([._]|$)|clinical.?significance/i.test(String(item.fieldPath||''))),clinvarPresentation=clinvarItem?evidencePresentation(clinvarItem,clinvarItem.value,{includeSource:true}):{display:'Not reported',tone:'missing',tooltip:'The result does not contain a ClinVar classification for this variant.'},clinvarTooltip=clinvarPresentation.tooltip;
  const revelItem=(domainGroups.prediction||[]).filter(item=>String(item.sourceId||'').toLowerCase().includes('revel')||String(item.fieldPath||'').toLowerCase().includes('revel')).sort((left,right)=>{const rank=item=>{const field=String(item.fieldPath||'').toLowerCase(),scope=item.scopeLabel==='Transcript'&&!item.unmatchedTranscript?0:favorTranscriptSummary(item)?20:10;if(field==='score'||field==='revel_score')return scope;if(field.includes('score')&&!field.includes('rank'))return scope+1;return scope+9};return rank(left)-rank(right)})[0]||null,revelPresentation=revelItem?evidencePresentation(revelItem,revelItem.value,{includeSource:true}):{display:'Not reported',tone:'missing',tooltip:'The result does not contain a REVEL score for this variant.'},revelTooltip=revelPresentation.tooltip,revelLabel=favorTranscriptSummary(revelItem)?'REVEL summary':'REVEL';
  const readCounts=sampleFacts.referenceReads==='Not available'&&sampleFacts.alternateReads==='Not available'?'Not available':`${sampleFacts.referenceReads} / ${sampleFacts.alternateReads}`,genotypeNotCalled=sampleFacts.zygosity==='Not called',genotypePhase=genotypeNotCalled&&sampleFacts.genotype!=='Not available'?sampleFacts.genotype:genotypeNotCalled?'Not called':sampleFacts.genotype==='Not available'&&sampleFacts.phase==='Not available'?'Not available':[sampleFacts.genotype,sampleFacts.phase].filter(value=>value!=='Not available').join(' · '),genotypePhaseTooltip=genotypeNotCalled?'The VCF does not contain a genotype call for this sample.':`The raw genotype and phase are from ${sampleFacts.sampleName||'the sample'}. Allele numbers use the original VCF ALT order. This row represents ALT ${variant.altIndex}.`,frequencyTooltip=frequency?evidenceValueTooltip(frequency,frequency.value,{includeSource:true}):'The result does not contain an overall population allele frequency.',groupMaximumTooltip=groupMaximum?evidenceValueTooltip(groupMaximum,groupMaximum.value,{includeSource:true}):'The result does not contain a population group-maximum allele frequency.';
  const options=unique.map((item,index)=>{const id=consequenceContextKey(item,index),featureId=consequenceValue(item,'feature_id','transcript_id','Feature','regulatory_feature_id','motif_feature_id'),featureType=readableTerm(consequenceValue(item,'feature_type')||'transcript'),label=[featureId||featureType,readableTerm(primaryConsequence(consequenceValue(item,'consequence_terms','Consequence'))),consequenceValue(item,'mane_select','MANE_SELECT','MANE')?'MANE Select':consequenceValue(item,'mane_plus_clinical','MANE_PLUS_CLINICAL')?'MANE Plus Clinical':consequenceValue(item,'canonical','CANONICAL')?'Canonical':''].filter(Boolean).join(' · ');return`<option value="${escapeHtml(id)}" ${id===selectedContext?'selected':''}>${escapeHtml(label)}</option>`}).join('');
  $('#variant-detail-title').textContent=`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`;
  const clinicalPriority=item=>{const field=String(item.fieldPath||'').toLowerCase();if(/significance|clnsig/.test(field))return 0;if(/review/.test(field))return 1;if(/condition|phenotype|disease/.test(field))return 2;if(/variant.?class/.test(field))return 3;if(/so.?accession|sequence.?ontology/.test(field))return 4;return 10},clinicalItems=[...(domainGroups.clinical||[])].sort((left,right)=>clinicalPriority(left)-clinicalPriority(right)),predictionItems=deduplicateMirroredEvidence(preferLocalPredictorEvidence(domainGroups.prediction||[])),rawSplicingItems=deduplicateMirroredEvidence(domainGroups.splicing||[]),spliceAiMaximum=spliceAiMaximumDeltaItem(rawSplicingItems),splicingItems=spliceAiMaximum?[spliceAiMaximum,...rawSplicingItems]:rawSplicingItems,conservationItems=conservationCandidates,functionalItems=deduplicateMirroredEvidence(domainGroups.functional||[]),populationItems=sortPopulationEvidence(deduplicateMirroredEvidence(domainGroups.population||[])),predictionSummary=predictionSummaryBar([...predictionItems.filter(item=>calibratedPredictorDefinition(item)?.id!=='revel'),...splicingItems]),technicalGroups=evidenceDomainOrder.filter(domain=>!['clinical','population','prediction','splicing','conservation','functional'].includes(domain)&&domainGroups[domain]?.length).map(domain=>[evidenceDomainTitles[domain],deduplicateMirroredEvidence(domainGroups[domain])]),provenance=`<div class="detail-provenance"><dl><div><dt>Assembly</dt><dd>GRCh38</dd></div><div><dt>Allele ID</dt><dd>${escapeHtml(detail.alleleId||row.alleleId)}</dd></div><div><dt>Sources</dt><dd>${escapeHtml([...new Set(evidence.map(item=>resourceTitle(item.sourceId)).filter(Boolean))].join(', ')||'Core annotation')}</dd></div><div><dt>Schema</dt><dd>${escapeHtml(displayDetailValue(detail.schemaVersion))}</dd></div></dl>${detail.evidenceTruncated?'<p class="detail-warning">Only the first 5,000 evidence fields are available.</p>':''}</div>`;
  const featureLabel=isTranscript?'selected transcript':`selected ${readableTerm(selectedFeatureType).toLowerCase()} feature`,sectionHeading=isTranscript?'Transcript & molecular effect':`${readableTerm(selectedFeatureType)} effect`,primarySelectedTerm=primaryConsequence(selectedTerms),secondarySelectedTerms=additionalConsequences(selectedTerms),consequenceDisplay=readableTerm(primarySelectedTerm)||readableTerm(row.consequence||variant.consequence),selectedImpact=readableTerm(consequenceValue(selected,'impact','IMPACT')||row.impact||variant.impact),consequenceTone=selectedImpact?variantFactTone('Impact',selectedImpact):'neutral',summaryRows=[
    variantSummaryRow([
      variantSummaryCell('Gene',gene,'neutral',`Gene symbol for the ${featureLabel} consequence, or its stable Ensembl identifier when no symbol is assigned.`),
      variantSummaryCell('Consequence',consequenceDisplay,consequenceTone,`Primary Sequence Ontology consequence for the ${featureLabel}.${secondarySelectedTerms.length?` Additional terms for this same feature: ${secondarySelectedTerms.map(readableTerm).join(', ')}.`:''}${selectedImpact?` VEP impact: ${selectedImpact}.`:''}`),
      variantSummaryCell('Zygosity',sampleFacts.zygosity,null,`This value describes how ${sampleFacts.sampleName||'the sample'} relates to the selected alternate allele. It uses GT and the original VCF ALT order.`),
      variantSummaryCell('ClinVar',clinvarPresentation.display,clinvarPresentation.tone,clinvarTooltip,clinvarPresentation.presentation)
    ]),
    variantSummaryRow([
      populationFrequencySummary(frequencyDisplay,groupMaximumDisplay,frequencyTooltip,groupMaximumTooltip),
      variantSummaryCell('Conservation',conservationSummaryPresentation.display,conservationSummaryPresentation.tone,conservationSummaryTooltip,conservationSummaryPresentation.presentation),
      variantSummaryCell(revelLabel,revelPresentation.display,revelPresentation.tone,revelTooltip,revelPresentation.presentation),
      predictionSummary
    ]),
    variantSummaryRow([
      variantSummaryCell('Genotype / phase',genotypePhase,null,genotypePhaseTooltip),
      variantSummaryPartsCell('Depth / GQ',[
        {value:sampleFacts.depth,tone:variantFactTone('Depth',sampleFacts.depth)},
        {value:sampleFacts.quality,tone:variantFactTone('GQ',sampleFacts.quality),separator:'/'}
      ],'FORMAT/DP gives the total read depth. FORMAT/GQ gives the genotype quality. AnnoCAT shows these values as context without an assay-specific quality profile.'),
      variantSummaryPartsCell('Allelic support',[
        {value:readCounts,tone:variantFactTone('Allelic support',sampleFacts.referenceReads)},
        {value:sampleFacts.alleleBalance,tone:variantFactTone('Allele balance',sampleFacts.alleleBalance),separator:'·'}
      ],'FORMAT/AD gives the reference and selected-alternate read depths. The percentage is the selected alternate fraction of all allele depths. AnnoCAT shows these values as context.'),
      variantSummaryPartsCell('VCF call',[
        {value:displayDetailValue(variant.filter),tone:variantFactTone('VCF FILTER',variant.filter)},
        {value:`QUAL ${displayDetailValue(variant.quality)}`,tone:variantFactTone('QUAL',variant.quality),separator:'·'}
      ],'VCF FILTER gives the record filter status. QUAL gives the site-level quality. AnnoCAT shows QUAL as context without a caller-specific quality profile.')
    ])
  ].join('');
  $('#variant-detail-body').innerHTML=`<section class="detail-overview"><div class="detail-links">${links.map(([label,url,title])=>`<a class="fui-button fui-button--small" href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtml(title)}">${escapeHtml(label)} ↗</a>`).join('')}</div><dl class="detail-summary fui-description-grid">${summaryRows}</dl></section>${groupedEvidenceSection('Clinical & population evidence',[['Clinical',clinicalItems],['Population',populationItems,'Not reported',{kind:'population',preferredSourceId:frequency?.sourceId}]],{className:'clinical-population-section',sectionKey:'clinical-population'})}<section class="detail-section transcript-context fui-accordion-stack"><details class="transcript-details collapsible-detail fui-accordion" data-variant-detail-section="transcript-molecular" ${variantDetailOpenSections.has('transcript-molecular')?'open':''}><summary><strong>${escapeHtml(sectionHeading)}</strong></summary><div class="transcript-context-body">${unique.length?`<select id="detail-consequence-select" class="fui-select" aria-label="Consequence context" title="Shows transcripts and features annotated for this allele. Transcripts present only in a supplementary source are not added.">${options}</select><div class="selected-transcript-card">${isTranscript?transcriptDetail(selected,metadata):nonTranscriptDetail(selected)}</div>`:'<p class="detail-empty">No molecular consequence was recorded.</p>'}${detail.consequencesTruncated?'<p class="detail-warning">Only the first 1,000 consequences are available.</p>':''}</div></details></section>${groupedEvidenceSection('Predictions & conservation',[['Prediction scores',predictionItems],['Splicing',splicingItems],['Conservation',conservationItems],['Functional annotation summaries',functionalItems]],{className:'prediction-evidence-section',sectionKey:'predictions-conservation'})}${groupedEvidenceSection('Technical details & provenance',technicalGroups,{className:'technical-provenance-section',extra:provenance,sectionKey:'technical-provenance'})}`;
  if(domainGroups.phenotype?.length){const detailFields=new Set(['phenotypeEvidenceDetails','geneMatchDetails']),profileItems=domainGroups.phenotype.filter(item=>!detailFields.has(String(item.fieldPath||''))),extra=geneMatchEvidenceExtra(domainGroups.phenotype)+phenotypeEvidenceExtra(domainGroups.phenotype),groups=profileItems.length?[['Gene evidence',profileItems]]:[];$('#variant-detail-body .transcript-context')?.insertAdjacentHTML('beforebegin',groupedEvidenceSection('Gene associations',groups,{className:'phenotype-evidence-section',sectionKey:'gene-associations',extra}))}
  $('#detail-consequence-select')?.addEventListener('change',event=>{detailConsequenceSelections.set(detail.alleleId||row.alleleId,event.target.value);renderVariantDetail(row,detail)});
  bindVariantDetailSectionState();
  revealVariantDetail();
}


function minimumGridWidth(key){if(key==='selection'||key==='candidate')return 36;if(['reference','alternate'].includes(key))return 40;if(key==='chromosome')return 44;if(key==='position')return 56;return 52}
function defaultGridWidth(key){if(key==='selection'||key==='candidate')return 36;if(key==='chromosome')return 56;if(key==='position')return 82;if(['reference','alternate'].includes(key))return 52;if(key==='impact')return 84;if(key==='gene')return 88;if(key==='canonical')return 76;if(key==='consequence')return 140;if(key.startsWith('evidence:'))return 118;return 108}
function gridWidthStorageKey(key){if(!key.startsWith('evidence:'))return key;const field=resultFieldCatalog[Number(key.slice(9))];return field?`evidence:${resultFieldIdentity(field)}`:key}
function gridWidth(key){const value=Number(resultGridWidths.get(gridWidthStorageKey(key)));return Number.isFinite(value)?Math.max(minimumGridWidth(key),Math.min(520,value)):defaultGridWidth(key)}
function fittedGridWidth(key,header,table){
  const columnIndex=[...header.parentElement.children].indexOf(header),context=document.createElement('canvas').getContext('2d');
  if(!context)return defaultGridWidth(key);
  const measure=(element,text,padding)=>{const style=getComputedStyle(element);context.font=style.font||`${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;return context.measureText(String(text||'').trim()).width+padding};
  const heading=header.querySelector('button')||header;
  let width=measure(heading,heading.textContent.replace(/[↕↑↓]/g,''),40);
  for(const row of table.tBodies[0]?.rows||[]){const cell=row.cells[columnIndex];if(cell)width=Math.max(width,measure(cell,cell.innerText,24))}
  return Math.ceil(Math.max(minimumGridWidth(key),Math.min(520,width)));
}
function tableValuePresentation(key,value,resolution=null,row=null){
  if(key==='impact'){const display=readableTerm(value),tone=/high/i.test(value)?'adverse':/moderate/i.test(value)?'caution':'neutral';return{display,tone,presentation:'text',description:'Predicted impact category from fastVEP; HIGH is most severe, followed by MODERATE, LOW, and MODIFIER.'}}
  if(key==='consequence'||key==='biotype')return{display:readableTerm(value),tone:'neutral',description:'Human-readable Sequence Ontology annotation.'};
  if(key.startsWith('evidence:')){const field=resultFieldCatalog[Number(key.slice(9))]||{},contextualField={...field,consequenceTerms:row?.consequenceTerms||row?.consequence||field.consequenceTerms},interpreted=evidencePresentation(contextualField,value,{includeSource:true,resolution});return{...interpreted,description:interpreted.tooltip}}
  return{display:displayDetailValue(value),tone:'neutral',description:coreColumnPresentation(key,key).description}
}
function observeResultGridViewport(tableWrap){
  if(resultGridViewportTarget!==tableWrap){
    resultGridViewportObserver?.disconnect();
    resultGridViewportTarget=tableWrap;
    resultGridViewportObserver=new ResizeObserver(entries=>entries.forEach(({target})=>target.style.setProperty('--fui-data-grid-viewport-width',`${target.clientWidth}px`)));
    resultGridViewportObserver.observe(tableWrap)
  }
  tableWrap.style.setProperty('--fui-data-grid-viewport-width',`${tableWrap.clientWidth}px`)
}
function enhanceResultGrid(){
  const table=document.querySelector('.table-wrap table');if(!table)return;
  const shown=displayColumns(),keys=['selection','candidate',...shown.map(([key])=>key)],tableWrap=table.parentElement,setTableWidth=()=>{const width=`${keys.reduce((total,key)=>total+gridWidth(key),0)}px`;table.style.width=width;tableWrap.style.setProperty('--result-table-width',width)};observeResultGridViewport(tableWrap);let colgroup=table.querySelector('colgroup');if(!colgroup){colgroup=document.createElement('colgroup');table.insertBefore(colgroup,table.firstChild)}colgroup.innerHTML=keys.map(key=>`<col data-grid-column="${escapeHtml(key)}" style="width:${gridWidth(key)}px">`).join('');setTableWidth();
  [...$('#head').children].forEach((header,index)=>{const key=keys[index];header.dataset.gridColumn=key;if(index<2)return;header.draggable=true;header.classList.add('draggable-column');header.addEventListener('dragstart',event=>{if(event.target.closest('.column-resizer')){event.preventDefault();return}header.dataset.columnDragged='true';event.dataTransfer.effectAllowed='move';event.dataTransfer.setData('text/plain',key);header.classList.add('column-dragging')});header.addEventListener('dragover',event=>{event.preventDefault();event.dataTransfer.dropEffect='move';header.classList.add('column-drag-target')});header.addEventListener('dragleave',()=>header.classList.remove('column-drag-target'));header.addEventListener('drop',event=>{event.preventDefault();header.classList.remove('column-drag-target');moveResultColumn(event.dataTransfer.getData('text/plain'),key)});header.addEventListener('dragend',()=>{header.classList.remove('column-dragging');document.querySelectorAll('.column-drag-target').forEach(item=>item.classList.remove('column-drag-target'));setTimeout(()=>delete header.dataset.columnDragged,0)});header.addEventListener('click',event=>{if(header.dataset.columnDragged==='true'){event.preventDefault();event.stopImmediatePropagation()}},true);if(!header.querySelector('.column-resizer')){const custom=resultGridWidths.has(gridWidthStorageKey(key)),title=custom?'Drag to resize. Double-click to restore the default width.':'Drag to resize. Double-click to fit loaded values.';header.insertAdjacentHTML('beforeend',`<span class="column-resizer" data-resize-column="${escapeHtml(key)}" title="${title}"></span>`)}});
  $('#head').querySelectorAll('[data-resize-column]').forEach(handle=>{handle.addEventListener('pointerdown',event=>{event.preventDefault();event.stopPropagation();const key=handle.dataset.resizeColumn,storageKey=gridWidthStorageKey(key),startX=event.clientX,startWidth=gridWidth(key),col=colgroup.querySelector(`[data-grid-column="${CSS.escape(key)}"]`),move=moveEvent=>{const width=Math.max(minimumGridWidth(key),Math.min(520,startWidth+moveEvent.clientX-startX));resultGridWidths.set(storageKey,String(width));col.style.width=`${width}px`;setTableWidth()},up=()=>{window.removeEventListener('pointermove',move);window.removeEventListener('pointerup',up);handle.title='Drag to resize. Double-click to restore the default width.';localStorage.setItem('annocat.resultColumnWidths',JSON.stringify(Object.fromEntries(resultGridWidths)))};window.addEventListener('pointermove',move);window.addEventListener('pointerup',up)});handle.addEventListener('dblclick',event=>{event.preventDefault();event.stopPropagation();const key=handle.dataset.resizeColumn,storageKey=gridWidthStorageKey(key),col=colgroup.querySelector(`[data-grid-column="${CSS.escape(key)}"]`);if(resultGridWidths.has(storageKey))resultGridWidths.delete(storageKey);else resultGridWidths.set(storageKey,String(fittedGridWidth(key,handle.parentElement,table)));col.style.width=`${gridWidth(key)}px`;setTableWidth();handle.title=resultGridWidths.has(storageKey)?'Drag to resize. Double-click to restore the default width.':'Drag to resize. Double-click to fit loaded values.';localStorage.setItem('annocat.resultColumnWidths',JSON.stringify(Object.fromEntries(resultGridWidths)))})});
  $('#rows').querySelectorAll('tr[data-allele-id]').forEach(element=>{const row=variants.find(item=>item.alleleId===element.dataset.alleleId);if(!row)return;shown.forEach(([key],index)=>{const cell=element.children[index+2],value=resultColumnRawValue(row,key);if(!cell)return;cell.classList.add('result-cell',`result-cell-${String(key).replace(/[^a-z0-9_-]/gi,'-')}`);if(key.startsWith('evidence:')){const evidenceIndex=Number(key.slice(9)),presentation=tableValuePresentation(key,value,row.evidenceResolution?.[evidenceIndex],row);applyGenericEvidenceCellPresentation(cell,`<span class="table-value ${evidenceClass(presentation)}" title="${escapeHtml(presentation.description)}">${escapeHtml(presentation.display)}</span>`)}else if(key==='consequence'||key==='biotype')cell.textContent=readableTerm(value)})});
}

function renderTable(event){renderTableBase(event);enhanceResultGrid()}

  return{
    evidenceFieldPresentation(...args){syncState();return evidenceFieldPresentation(...args)},
    consequenceTerms(...args){syncState();return consequenceTerms(...args)},
    evidenceValuePresentation(...args){syncState();return evidenceValuePresentation(...args)},
    evidencePresentation(...args){syncState();return evidencePresentation(...args)},
    predictionSummaryBar(...args){syncState();return predictionSummaryBar(...args)},
    primaryAlleleFrequency(...args){syncState();return primaryAlleleFrequency(...args)},
    preferredConservationSummary(...args){syncState();return preferredConservationSummary(...args)},
    preferredConsequence(...args){syncState();return preferredConsequence(...args)},
    evidenceForSelectedConsequence(...args){syncState();return evidenceForSelectedConsequence(...args)},
    renderVariantDetail(...args){syncState();return renderVariantDetail(...args)},
    renderTable(...args){syncState();return renderTable(...args)}
  }
}
