export function createVariantPresentation({
  $,columns,escapeHtml,prototypeIcon,resultFieldIdentity,fieldSourceIs,resourceTitle,
  coreColumnPresentation,displayColumns,moveResultColumn,decodeEvidenceValue,resultColumnRawValue,
  renderTableBase,revealVariantDetail,consequenceValue,usefulVariantLinks,displayDetailValue,
  dbnsfpPredictionValue,evidenceFieldPresentationBase,getState
}){
  let resultFieldCatalog=[],evidenceCalibrations={interpretationPolicy:{},predictors:[],calibrations:[],displayPolicies:{}},variants=[],resultAlignmentGroups=[];
  function syncState(){({resultFieldCatalog,evidenceCalibrations,variants,resultAlignmentGroups}=getState())}

const detailConsequenceSelections=new Map();
const variantDetailOpenSections=new Set(['clinical-population']);
const resultGridWidths=new Map((()=>{try{return Object.entries(JSON.parse(localStorage.getItem('annocat.resultColumnWidths')||'{}'))}catch{return[]}})());
const dbnsfpTranscriptMetadata=new Set(['Ensembl_geneid','Ensembl_transcriptid','Ensembl_proteinid','Uniprot_acc','HGVSc_VEP','HGVSp_VEP','APPRIS','GENCODE_basic','TSL','VEP_canonical','aaref','aaalt','aapos','genename']);
function dbnsfpVariantLevelField(field){return/^(CADD_|GERP\+\+_|phyloP|phastCons|SiPhy|LINSIGHT|GenoCanyon|fitCons|Eigen)/i.test(String(field||''))}

function evidenceReadingGuide(source,field){
  const id=String(source||'').toLowerCase(),key=String(field||''),lower=key.toLowerCase();
  const predictor=calibratedPredictorDefinition({sourceId:source,fieldPath:field});
  if(predictor?.calibrationStatus==='published')return`${predictor.scoreIdentity}. AnnoCat shows the published calibrated range only when its variant-type requirements are met. The range is interpretive context, not a classification.`;
  if(predictor?.calibrationStatus==='unverified')return`${predictor.scoreIdentity}. AnnoCat treats this score as contextual and does not apply a clinical calibration.`;
  if(lower.includes('apc')&&lower.includes('protein'))return'FAVOR PHRED-ranked protein-function summary; 10 is approximately the top 10%, 20 the top 1%, and 30 the top 0.1%. Higher means stronger predicted functional effect, not a diagnosis.';
  if(lower.includes('apc')&&lower.includes('conservation'))return'FAVOR PHRED-ranked conservation summary; 10 is approximately the top 10%, 20 the top 1%, and 30 the top 0.1%. Higher means stronger evolutionary constraint.';
  if(lower.includes('apc')&&(lower.includes('epigen')||lower.includes('transcription_factor')||lower.includes('transcription-factor')))return'FAVOR PHRED-ranked regulatory-context summary. Higher means stronger evidence that the surrounding region is biologically active; it does not prove that this allele changes regulation.';
  if(lower.includes('apc')&&lower.includes('mappability'))return'FAVOR summary of sequence uniqueness. Interpret this as technical context rather than pathogenicity evidence.';
  if(lower.includes('apc')&&(lower.includes('proximity')||lower.includes('distance')))return'FAVOR proximity summary. Nearby genes are candidates, but the nearest gene is not necessarily the affected gene.';
  if(lower.includes('apc')&&(lower.includes('mutation')||lower.includes('diversity')||lower.includes('microrna')))return'FAVOR regional-context summary. Higher values indicate a stronger signal in this category, not pathogenicity by themselves.';
  if(lower.includes('mappability')||lower.includes('umap')||lower.includes('bismap'))return'0–1 sequence-uniqueness measure; values near 1 are easier to map reliably, while low values warrant review of read and mapping quality.';
  if(lower.includes('min_dist_tss'))return'Distance in bases to the nearest transcription start site. Smaller values mean closer, but proximity alone does not establish a target gene.';
  if(lower.includes('min_dist_tse')||lower.includes('min_dist_tes'))return'Distance in bases to the nearest transcription end site. Smaller values mean closer.';
  if(lower.includes('remap')&&lower.includes('overlap_tf'))return'Number of transcription factors observed binding at this location. More overlap suggests regulatory activity, but tissue and allele-specific effects still matter.';
  if(lower.includes('remap')&&lower.includes('overlap_cl'))return'Number of transcription-factor and cell-line combinations observed at this location. A larger count indicates broader experimental overlap.';
  if(lower.includes('ccre'))return'ENCODE candidate regulatory-element annotation. It describes the surrounding region and does not prove that this allele changes its activity.';
  if(lower.includes('chromhmm')||lower.includes('chromatin'))return'Chromatin-state evidence across assayed cells or tissues. Relevance depends on whether those biological contexts match the case.';
  if(lower.includes('sift_score'))return'0–1; scores at or below 0.05 suggest a deleterious effect.';
  if(lower.includes('sift_pred'))return'D = deleterious; T = tolerated.';
  if(lower.includes('polyphen')&&lower.includes('score'))return'0–1; higher scores suggest a damaging protein effect.';
  if(lower.includes('polyphen')&&lower.includes('pred'))return'D = probably damaging; P = possibly damaging; B = benign.';
  if(lower.includes('alphamissense_score')||id.includes('alphamissense')&&lower.includes('score'))return'0–1; below 0.34 is typically benign and above 0.56 pathogenic.';
  if(lower.includes('alphamissense_pred')||id.includes('alphamissense')&&lower.includes('pred'))return'B = benign; A = uncertain; P = pathogenic.';
  if(lower.includes('revel')||id.includes('revel')&&lower.includes('score'))return'0–1; higher scores suggest pathogenicity; thresholds depend on use case.';
  if((lower.includes('cadd')||id.includes('cadd'))&&lower.includes('phred'))return'Higher is more deleterious; 10 is approximately the top 10% and 20 the top 1%.';
  if(lower.includes('primateai_score')||id.includes('primateai')&&lower.includes('score'))return'Original PrimateAI score from 0 to 1. The displayed evidence band uses the versioned global ClinGen calibration.';
  if(lower.includes('primateai_pred')||id.includes('primateai')&&lower.includes('pred'))return'D/T is dbNSFP’s fixed 0.803 cutoff. Use the score and calibrated interval for clinical evidence strength.';
  if(id.includes('spliceai')||lower.startsWith('ds_'))return'Individual 0–1 splice-effect component. AnnoCat applies the published calibrated interpretation only to the maximum of the four delta scores.';
  if(id.includes('phylop')||lower.includes('phylop'))return'Positive values indicate conservation; negative values indicate accelerated change.';
  if(id.includes('gerp')||lower.includes('gerp'))return'Higher positive values indicate stronger constraint; values above 2 are often conserved.';
  if(lower==='af'||lower.includes('allele_frequency'))return'Population frequency; lower is rarer, but rarity alone is not pathogenicity.';
  if(lower.includes('significance'))return'Interpret with ClinVar review status and supporting submissions.';
  if(lower==='tsl')return'Level 1 has the strongest transcript support; missing means no level was reported.';
  if(lower==='appris')return'Principal marks a preferred protein isoform; alternative marks another supported isoform.';
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
function evidenceFieldLeaf(item){return String(item?.fieldPath||'').split(/[.\[\]]/).filter(Boolean).pop()||''}
function calibratedPredictorDefinition(item){
  const field=evidenceFieldLeaf(item).toLowerCase();
  return(evidenceCalibrations.predictors||[]).find(predictor=>(predictor.matches||[]).some(match=>(match.sourceIds||[]).some(sourceId=>fieldSourceIs(item,sourceId))&&(match.fieldNames||[]).some(fieldName=>String(fieldName).toLowerCase()===field)))||null
}
function normalizedConsequenceTerms(value){
  const values=Array.isArray(value)?value:[value];
  return new Set(values.flatMap(item=>String(item||'').split(/[,&|;]/)).map(item=>item.trim().toLowerCase().replace(/\s+/g,'_')).filter(Boolean))
}
function predictorApplicability(predictor,item){
  const required=predictor?.variantClasses||[],excluded=predictor?.excludedVariantClasses||[],observed=normalizedConsequenceTerms(item?.consequenceTerms);
  if(excluded.length){
    if(!observed.size)return{applies:false,note:`Published calibration excludes ${excluded.map(readableTerm).join(', ')}; no selected consequence context was available.`};
    const matched=excluded.filter(term=>observed.has(String(term).toLowerCase()));
    if(matched.length)return{applies:false,note:`Published calibration was not applied to this ${matched.map(readableTerm).join(', ')}; canonical splice-site variants require separate loss-of-function assessment.`}
  }
  if(!required.length||required.includes('any'))return{applies:true,note:''};
  if(!observed.size)return{applies:false,note:`Published calibration applies only to ${required.map(readableTerm).join(', ')}; no selected consequence context was available.`};
  if(required.some(term=>observed.has(String(term).toLowerCase())))return{applies:true,note:''};
  return{applies:false,note:`Published calibration applies only to ${required.map(readableTerm).join(', ')} and was not applied to ${[...observed].map(readableTerm).join(', ')}.`}
}
function calibratedEvidenceBand(calibrationId,score){
  const calibration=(evidenceCalibrations.calibrations||[]).find(item=>item.id===calibrationId);
  if(!calibration||!Number.isFinite(score))return null;
  const within=(band,key,comparison)=>band[key]===undefined||comparison(score,Number(band[key])),band=calibration.bands.find(item=>within(item,'minimumInclusive',(value,limit)=>value>=limit)&&within(item,'minimumExclusive',(value,limit)=>value>limit)&&within(item,'maximumInclusive',(value,limit)=>value<=limit)&&within(item,'maximumExclusive',(value,limit)=>value<limit));
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
  if(!applicability.applies)return{predictor,applicable:false,note:applicability.note};
  const evidenceBand=calibratedEvidenceBand(predictor.calibrationId,score);
  return{predictor,applicable:true,evidenceBand:evidenceBand?{...evidenceBand,predictorId:predictor.id,predictorLabel:predictor.label,evidenceGroup:predictor.evidenceGroup,role:predictor.role,scoreIdentity:predictor.scoreIdentity}:null,note:evidenceBand?'':'The score did not fall in a published calibrated interval.'}
}
function alleleFrequencyField(source,field){const id=String(source||'').toLowerCase(),lower=String(field||'').toLowerCase(),population='all|global|grpmax|afr|amr|asj|eas|fin|mid|nfe|oth|remaining|sas|exac|tgp|esp\\d*|1000g';return lower==='af'||lower.includes('allele_frequency')||/(^|[._])faf\d*([._]|$)/.test(lower)||new RegExp(`(${population})_?af$`).test(lower)||new RegExp(`(^|[._])af[._]?(${population})$`).test(lower)||(id.includes('gnomad')&&/(^|[._])af([._]|$)/.test(lower))}
function alleleFrequencyTone(value){
  const text=String(value??'').trim(),lower=text.toLowerCase();
  if(!text||text==='.'||text==='-'||/not available|not reported/.test(lower))return'missing';
  const number=Number(text.match(/-?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?/i)?.[0]);
  if(!Number.isFinite(number)||number<0||number>1)return'neutral';
  if(number<.01)return evidenceCalibrations.displayPolicies?.alleleFrequency?.rareTone||'informative';
  return'neutral'
}
function primaryAlleleFrequency(items){const frequencies=items.filter(item=>evidenceDomain(item)==='population'&&alleleFrequencyField(item.sourceId,item.fieldPath)),priority=item=>{const field=String(item.fieldPath||'').toLowerCase().replace(/[^a-z0-9]/g,'');if(field==='allaf'||field==='af'||field==='allelefrequency')return 0;if(field==='globalmaf')return 1;if(field==='grpmaxaf'||field.includes('maxfrequency'))return 2;if(field==='faf95')return 3;if(field==='faf99')return 4;return 10};return frequencies.sort((left,right)=>priority(left)-priority(right))[0]||null}
function groupMaximumAlleleFrequency(items){const rank=item=>{const field=String(item.fieldPath||'').toLowerCase().replace(/[^a-z0-9]/g,'');if(field.includes('grpmaxaf')||field.includes('groupmaxaf')||field.includes('popmaxaf'))return 0;if(field.includes('maxfrequency')||field.includes('maxaf'))return 1;return 10};return items.filter(item=>evidenceDomain(item)==='population'&&alleleFrequencyField(item.sourceId,item.fieldPath)&&rank(item)<10).sort((left,right)=>rank(left)-rank(right))[0]||null}
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
  if(/^(allaf|af|overallaf|globalaf|allelefrequency)$/.test(field))return'overall';
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
  const interpreted=evidenceValuePresentation(item),tooltip=evidenceValueTooltip(item,item.value,{includeSource:true}),sourcePrefix=showSource?`${resourceTitle(item.sourceId)} · `:'';
  return`<div class="annotation-row fui-key-value-row tone-${interpreted.tone}${isMaximum?' group-maximum-row':''}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(`${sourcePrefix}${label}`)}</strong>${isMaximum?'<small>Highest</small>':''}</span><b>${escapeHtml(interpreted.display)}</b></div>`
}
function populationEvidenceSubgroup(items,preferredSourceId='',empty='None reported'){
  if(!items.length)return`<div class="key-evidence-subgroup population-evidence" data-evidence-group="population"><div class="key-evidence-subheading fui-section-heading"><strong>Population</strong></div><div class="annotation-list fui-key-value-list"><div class="key-evidence-empty">${escapeHtml(empty)}</div></div></div>`;
  const sourceKey=value=>String(value||'').toLowerCase(),preferredKey=sourceKey(preferredSourceId),primarySourceId=items.find(item=>sourceKey(item.sourceId)===preferredKey)?.sourceId||items.find(item=>sourceKey(item.sourceId).includes('gnomad'))?.sourceId||items[0].sourceId,primaryKey=sourceKey(primarySourceId),primaryItems=items.filter(item=>sourceKey(item.sourceId)===primaryKey);
  const findKind=kind=>primaryItems.find(item=>populationFieldKind(item)===kind),overall=findKind('overall'),alleleCount=findKind('alleleCount'),alleleNumber=findKind('alleleNumber'),homozygotes=findKind('homozygotes'),reported=item=>item&&evidenceValuePresentation(item).tone!=='missing';
  const ancestryRows=primaryItems.map(item=>{const ancestry=populationAncestryEntry(item);if(!ancestry)return null;const number=Number(decodeEvidenceValue(item.value));return{...ancestry,item,number:Number.isFinite(number)&&number>=0&&number<=1?number:Number.NEGATIVE_INFINITY}}).filter(Boolean).sort((left,right)=>right.number-left.number||left.label.localeCompare(right.label));
  const explicitMaximum=findKind('groupMaximum'),maximumLabelItem=findKind('groupMaximumLabel'),derivedMaximum=ancestryRows.find(row=>Number.isFinite(row.number)&&row.number!==Number.NEGATIVE_INFINITY),maximumItem=reported(explicitMaximum)?explicitMaximum:derivedMaximum?.item,maximumPresentation=maximumItem?evidenceValuePresentation(maximumItem):{display:'Not reported',tone:'missing'},maximumCode=populationGroupCode(decodeEvidenceValue(maximumLabelItem?.value))||derivedMaximum?.code||'',maximumLabel=maximumCode?populationAncestryLabels[maximumCode]:maximumLabelItem?readableTerm(displayDetailValue(maximumLabelItem.value)):'';
  const overallPresentation=overall?evidenceValuePresentation(overall):{display:'Not reported',tone:'missing'},countPresentation=populationCountPresentation(alleleCount),numberPresentation=populationCountPresentation(alleleNumber),homozygotePresentation=populationCountPresentation(homozygotes),countReported=countPresentation.tone!=='missing',numberReported=numberPresentation.tone!=='missing',countAndNumber={display:countReported&&numberReported?`${countPresentation.display} of ${numberPresentation.display}`:countReported?`${countPresentation.display}; total not reported`:numberReported?`Alternate count not reported; ${numberPresentation.display} measured`:'Not reported',tone:countReported||numberReported?'neutral':'missing'},maximumDisplay={...maximumPresentation,display:maximumLabel&&maximumPresentation.tone!=='missing'?`${maximumPresentation.display} · ${maximumLabel}`:maximumPresentation.display};
  const sourceTitle=resourceTitle(primarySourceId),overallTooltip=overall?evidenceValueTooltip(overall,overall.value,{includeSource:true}):`No overall allele frequency was reported by ${sourceTitle}.`,maximumTooltip=maximumItem?`${evidenceValueTooltip(maximumItem,maximumItem.value,{includeSource:true})}${maximumLabel?` Highest group: ${maximumLabel}.`:''}`:`No ancestry-group maximum was reported by ${sourceTitle}.`,countTooltip=countReported||numberReported?`Alternate allele copies observed: ${countPresentation.display}. Total alleles with usable genotype calls: ${numberPresentation.display}. Source: ${sourceTitle}.`:`No allele counts were reported by ${sourceTitle}.`,homozygoteTooltip=homozygotes?evidenceValueTooltip(homozygotes,homozygotes.value,{includeSource:true}):`No alternate-homozygote count was reported by ${sourceTitle}.`;
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
  if(!vector.reported.length)return{display:'Not reported',tone:'missing',summaryNote:`No value reported across ${vector.all.length} source transcript entries`};
  const distinct=[...new Set(vector.reported)];
  if(distinct.length===1){
    const scalar=evidenceValuePresentation(item,distinct[0]);
    return{...scalar,summaryNote:`Same reported value across ${vector.reported.length} source transcript entr${vector.reported.length===1?'y':'ies'}`}
  }
  const numbers=distinct.map(Number);
  if(numbers.every(Number.isFinite)){
    const minimum=Math.min(...numbers),maximum=Math.max(...numbers),format=number=>Number.isInteger(number)?String(number):plainDecimalNotation(String(Number(number.toPrecision(6)))),display=`${format(minimum)}–${format(maximum)}`,presentations=numbers.map(number=>evidenceValuePresentation(item,String(number))),calibrated=presentations.filter(presentation=>presentation.evidenceBand||presentation.predictorInterpretation);
    if(calibrated.length){
      const labels=new Set(presentations.map(presentation=>presentation.evidenceBand?.label||''));
      if(calibrated.length===presentations.length&&labels.size===1&&!labels.has('')){
        const scalar=presentations[0];
        return{...scalar,display,summaryNote:`${scalar.evidenceBand.label} · Range across ${vector.reported.length} reported source transcript entries`}
      }
      return{display,tone:'neutral',summaryNote:`Range across ${vector.reported.length} reported source transcript entries spans multiple or unresolved calibrated intervals`,predictorInterpretation:{note:'No single calibrated range was assigned because the source transcript scores do not resolve to one interval.'}}
    }
    const tones=new Set(presentations.map(presentation=>presentation.tone));
    return{...presentations[0],display,tone:tones.size===1?presentations[0].tone:'neutral',summaryNote:`Range across ${vector.reported.length} reported source transcript entries`}
  }
  const values=distinct.map(part=>evidenceValuePresentation(item,part).display),shown=values.slice(0,3),remaining=values.length-shown.length;
  return{display:`${shown.join(', ')}${remaining?` +${remaining} more`:''}`,tone:'neutral',summaryNote:`Distinct values across ${vector.reported.length} reported source transcript entries`}
}
function evidenceValuePresentation(item,value=item.value){
  value=decodeEvidenceValue(value);
  const vectorPresentation=evidenceVectorPresentation(item,value);
  if(vectorPresentation)return vectorPresentation;
  const source=String(item.sourceId||'').toLowerCase(),field=String(item.fieldPath||''),lower=field.toLowerCase(),raw=Array.isArray(value)?value.map(item=>displayDetailValue(item)).join(', '):String(value??'');
  if(!raw||raw==='.'||raw==='-')return{display:'Not reported',tone:'missing'};
  const prediction=dbnsfpPredictionValue(field,raw);
  if(prediction!==raw){const lowered=prediction.toLowerCase(),tone=/deleterious|damaging|pathogenic/.test(lowered)?'adverse':/uncertain|ambiguous|possibly|intermediate/.test(lowered)?'caution':/benign|tolerated|yes/.test(lowered)?'reassuring':'neutral';return{display:prediction,tone}}
  if(/(?:^|[._])(pred|prediction)(?:$|[._])/.test(lower)){const display=readableTerm(raw).replace(/\bambiguous\b/gi,'Uncertain'),lowered=display.toLowerCase(),tone=/deleterious|damaging|pathogenic/.test(lowered)?'adverse':/uncertain|possibly|intermediate/.test(lowered)?'caution':/benign|tolerated/.test(lowered)?'reassuring':'neutral';return{display,tone}}
  if(lower==='appris')return{display:raw.replace(/^principal(\d+)$/i,'Principal isoform $1').replace(/^alternative(\d+)$/i,'Alternative isoform $1'),tone:/^principal/i.test(raw)?'informative':'neutral'};
  if(lower==='tsl')return{display:raw==='1'?'Level 1 (best supported)':`Level ${raw}`,tone:raw==='1'?'reassuring':'neutral'};
  if(lower==='aaref'||lower==='aaalt'){const amino={A:'Alanine',R:'Arginine',N:'Asparagine',D:'Aspartic acid',C:'Cysteine',E:'Glutamic acid',Q:'Glutamine',G:'Glycine',H:'Histidine',I:'Isoleucine',L:'Leucine',K:'Lysine',M:'Methionine',F:'Phenylalanine',P:'Proline',S:'Serine',T:'Threonine',W:'Tryptophan',Y:'Tyrosine',V:'Valine',X:'Unknown'};return{display:amino[raw]?`${amino[raw]} (${raw})`:raw,tone:'neutral'}}
  if(lower.includes('ccre')&&lower.includes('annotation')){const labels={PLS:'Promoter-like',pELS:'Proximal enhancer-like',dELS:'Distal enhancer-like','CA-CTCF':'Accessible CTCF-bound','CA-H3K4me3':'Accessible H3K4me3','CA-TF':'Accessible transcription-factor site',CA:'Chromatin accessible','TF Only':'Transcription-factor only','CTCF-Bound':'CTCF-bound'};return{display:raw.split(/[,;]/).map(value=>labels[value.trim()]||readableTerm(value)).join(', '),tone:'informative'}}
  if(lower.includes('significance')){const display=readableTerm(raw),tone=/pathogenic/i.test(raw)&&!/conflict/i.test(raw)?'adverse':/uncertain|conflict/i.test(raw)?'caution':/benign/i.test(raw)?'reassuring':'neutral';return{display,tone}}
  const number=Number(raw);
  if(Number.isFinite(number)){
    const calibrated=calibratedPredictorInterpretation(item,number);
    if(calibrated){
      const display=plainDecimalNotation(raw),fieldName=String(field).toLowerCase(),contextTone=(fieldName.includes('cadd')||source.includes('cadd'))?(evidenceCalibrations.displayPolicies?.cadd?.tone||'informative'):(fieldName.includes('phylop')||source.includes('phylop'))?number>0?(evidenceCalibrations.displayPolicies?.conservation?.conservedTone||'informative'):number<0?(evidenceCalibrations.displayPolicies?.conservation?.acceleratedTone||'caution'):'neutral':(fieldName.includes('gerp')||source.includes('gerp'))?number>0?(evidenceCalibrations.displayPolicies?.conservation?.conservedTone||'informative'):number<0?(evidenceCalibrations.displayPolicies?.conservation?.acceleratedTone||'caution'):'neutral':'neutral';
      if(!calibrated.evidenceBand)return{display,tone:contextTone,summaryNote:calibrated.note,predictorInterpretation:calibrated};
      const rank=(fieldName.includes('cadd')||source.includes('cadd'))?phredRankLabel(number):'',summaryNote=[calibrated.evidenceBand.label,rank?`CADD PHRED rank: ${rank}`:''].filter(Boolean).join(' · ');
      return{display,tone:calibrated.evidenceBand.tone,evidenceBand:calibrated.evidenceBand,summaryNote,predictorInterpretation:calibrated}
    }
    if(lower.includes('apc')){const rank=phredRankLabel(number);return{display:[raw,rank].filter(Boolean).join(' · '),tone:'informative'}}
    if(lower.includes('mappability')||lower.includes('umap')||lower.includes('bismap'))return{display:raw,tone:number<.5?'caution':number>=.9?'reassuring':'neutral'};
    if(lower.includes('min_dist_tss')||lower.includes('min_dist_tse')||lower.includes('min_dist_tes'))return{display:number>=1000?`${(number/1000).toLocaleString(undefined,{maximumFractionDigits:2})} kb`:`${number.toLocaleString()} bp`,tone:'neutral'};
    if(lower.includes('sift_score'))return{display:raw,tone:number<=.05?'adverse':'reassuring'};
    if(lower.includes('polyphen')&&lower.includes('score'))return{display:raw,tone:number>=.85?'adverse':number>=.45?'caution':'reassuring'};
    if(lower.includes('alphamissense_score')||source.includes('alphamissense')&&lower.includes('score'))return{display:raw,tone:number>=.564?'adverse':number>=.34?'caution':'reassuring'};
    if(lower.includes('revel')||source.includes('revel')&&lower.includes('score'))return{display:raw,tone:'neutral'};
    if((lower.includes('cadd')||source.includes('cadd'))&&lower.includes('phred')){const rank=phredRankLabel(number);return{display:plainDecimalNotation(raw),tone:evidenceCalibrations.displayPolicies?.cadd?.tone||'informative',summaryNote:rank?`CADD PHRED rank: ${rank}`:''}}
    if(lower.includes('primateai_score')||source.includes('primateai')&&lower.includes('score'))return{display:raw,tone:'neutral'};
    if(source.includes('spliceai')||lower.startsWith('ds_'))return{display:raw,tone:number>=.2?'informative':'neutral'};
    if(source.includes('phylop')||lower.includes('phylop'))return{display:raw,tone:number>0?(evidenceCalibrations.displayPolicies?.conservation?.conservedTone||'informative'):number<0?(evidenceCalibrations.displayPolicies?.conservation?.acceleratedTone||'caution'):'neutral'};
    if(source.includes('gerp')||lower.includes('gerp'))return{display:raw,tone:number>0?(evidenceCalibrations.displayPolicies?.conservation?.conservedTone||'informative'):number<0?(evidenceCalibrations.displayPolicies?.conservation?.acceleratedTone||'caution'):'neutral'};
    if(alleleFrequencyField(source,field)){const display=plainDecimalNotation(raw),rarityNote=number<=.0001?'Very rare; rarity is supporting context, not a pathogenic classification.':'';return{display,tone:alleleFrequencyTone(raw),summaryNote:rarityNote}}
  }
  return{display:readableTerm(raw),tone:'neutral'};
}

function evidenceValueTooltip(item,value=item.value,{includeSource=false,resolution=null}={}){
  const presentation=evidenceFieldPresentation(item),interpreted=evidenceValuePresentation(item,value),raw=displayDetailValue(value),plainRaw=plainDecimalNotation(raw),field=String(item.fieldPath||'').toLowerCase(),number=Number(raw),family=predictionSummaryFamily(item),categorical=family&&/(?:^|[._])(pred|prediction)(?:$|[._])/.test(field),parts=[];
  if(categorical)parts.push(`Prediction: ${interpreted.display}.`);
  else if(Number.isFinite(number)&&calibratedPredictorDefinition(item)){
    parts.push(`Score: ${plainDecimalNotation(raw)}.`);
    if(interpreted.evidenceBand){const threshold=calibrationThresholdLabel(interpreted.evidenceBand);parts.push(`Interpretation: ${conciseEvidenceBandLabel(interpreted.evidenceBand.label)}${threshold?` (${threshold})`:''}.`)}
    else parts.push(`Interpretation: ${interpreted.predictorInterpretation?.applicable===false?'No calibrated threshold applies to this consequence.':'No calibrated direction is available.'}`)
  }else if(evidenceVectorParts(raw))parts.push(`Values: ${interpreted.display}.`);
  else if(alleleFrequencyField(item.sourceId,item.fieldPath))parts.push(`Frequency: ${interpreted.display}. Lower values are rarer; frequency alone does not determine pathogenicity.`);
  else{
    parts.push(`${presentation.label}: ${interpreted.display}.`);
    const guide=presentation.readingGuide||presentation.baseDescription;
    if(guide)parts.push(guide)
  }
  if(resolution?.kind==='ambiguous')parts.push('Selected transcript unavailable in this source; showing its transcript range.');
  else if(resolution?.kind==='invalid_vector')parts.push('Transcript alignment unavailable; showing the source range.');
  if(includeSource)parts.push(`Source: ${resourceTitle(item.sourceId)}.`);
  return parts.filter(Boolean).join(' ')
}

function selectedTranscriptEvidence(items,transcriptId){
  const normalizeTranscript=value=>String(value||'').trim().split('.')[0],target=normalizeTranscript(transcriptId);
  return items.flatMap(item=>{
    let group=resultAlignmentGroups.find(group=>group.sourceId===item.sourceId&&group.scope===item.scope&&group.fields?.includes(item.fieldPath));
    if(!group&&String(item.sourceId).toLowerCase()==='dbnsfp'&&!dbnsfpVariantLevelField(item.fieldPath))group={keyField:'Ensembl_transcriptid',separator:';'};
    if(!group)return[{...item,scopeLabel:item.scope==='transcript'?'Transcript':'Variant'}];
    const transcriptField=items.find(candidate=>candidate.sourceId===item.sourceId&&candidate.scope===item.scope&&candidate.fieldPath===group.keyField&&typeof candidate.value==='string'),separator=group.separator||';',transcripts=transcriptField?transcriptField.value.split(separator).map(normalizeTranscript):[],matches=transcripts.map((value,index)=>value===target?index:-1).filter(index=>index>=0),index=matches.length===1?matches[0]:-1;
    if(index<0)return[{...item,scopeLabel:'Source transcripts',unmatchedTranscript:true}];
    if(typeof item.value==='string'&&item.value.includes(';')){
      const values=item.value.split(separator);
      if(values.length===transcripts.length)return[{...item,value:values[index],scopeLabel:'Transcript'}];
      return[{...item,scopeLabel:'Source transcripts',unmatchedTranscript:true}]
    }
    return[{...item,scopeLabel:'Transcript'}];
  });
}

const evidenceDomainTitles={key:'Key evidence',clinical:'Clinical evidence',population:'Population frequency',prediction:'Prediction scores',splicing:'Splicing',conservation:'Conservation',regulatory:'Regulatory and noncoding',gene:'Gene relationships',technical:'Technical reliability',regional:'Regional context',other:'Other evidence'};
const evidenceDomainOrder=['clinical','population','prediction','splicing','conservation','regulatory','gene','technical','regional','other'];
function evidenceDomain(item){
  const source=String(item?.sourceId||'').toLowerCase(),field=String(item?.fieldPath||'').toLowerCase(),text=`${source} ${field}`;
  if(alleleFrequencyField(source,field))return'population';
  if(/clinvar|clingen|clnsig|clinical|review.?status|condition|phenotype|disease/.test(text))return'clinical';
  if(/gnomad|topmed|bravo|1000.?genomes|population|allele.?frequency|(^|[._])af([._]|$)|faf|nhomalt|allele.?count|allele.?number/.test(text))return'population';
  if(/splice|(^|[._])ds_(ag|al|dg|dl)([._]|$)/.test(text))return'splicing';
  if(/phylop|phastcons|gerp|conservation|conserved/.test(text))return'conservation';
  if(/mappability|umap|bismap|low.?complexity|repeat.?mask|segmental.?dup/.test(text))return'technical';
  if(/ccre|enhancer|promoter|remap|transcription.?factor|chromhmm|chromatin|epigen|histone|eqtl|sqtl|caqtl|microrna|mirna|cage|dnase|atac/.test(text))return'regulatory';
  if(/target.?gene|genehancer|nearest.?gene|min_dist_(tss|tse|tes)|distance.*(gene|coding|tss|tes)|proximity/.test(text))return'gene';
  if(/mutation.?density|variant.?density|nucleotide.?diversity|nucdiv|recombination|mutation.?rate|genomic.?context/.test(text))return'regional';
  if(/sift|polyphen|alphamissense|primateai|revel|meta.?svm|mutationtaster|mutationassessor|protein.?function|amino.?acid/.test(text))return'prediction';
  if(/cadd|fathmm|linsight|jarvis|remm|ncboost|macie|gnocchi|ncer|gpn|deleterious|pathogenicity.?score/.test(text))return'prediction';
  return'other';
}
function predictionSummaryFamily(item){
  const source=String(item?.sourceId||'').toLowerCase(),field=String(item?.fieldPath||'').toLowerCase(),text=`${source} ${field}`;
  const definitions=[
    ['alphamissense','AlphaMissense',/alphamissense/],
    ['primateai','PrimateAI',/primateai/],
    ['revel','REVEL',/revel/],
    ['cadd','CADD',/cadd/],
    ['sift','SIFT',/sift/],
    ['polyphen-hdiv','PolyPhen-2 HDIV',/polyphen.*hdiv/],
    ['polyphen-hvar','PolyPhen-2 HVAR',/polyphen.*hvar/],
    ['polyphen','PolyPhen-2',/polyphen/],
    ['spliceai','SpliceAI',/spliceai|(^|[ ._])ds_(ag|al|dg|dl)([ ._]|$)/],
    ['metasvm','MetaSVM',/meta.?svm/],
    ['mutationtaster','MutationTaster',/mutationtaster/],
    ['mutationassessor','MutationAssessor',/mutationassessor/],
    ['fathmm','FATHMM',/fathmm/]
  ];
  const match=definitions.find(([, ,pattern])=>pattern.test(text));
  return match?{key:match[0],label:match[1]}:null
}
function predictionSummaryBar(items){
  const toneRank={missing:0,neutral:1,informative:1,reassuring:2,caution:3,adverse:4},families=new Map();
  items.forEach(item=>{
    const family=predictionSummaryFamily(item);if(!family)return;
    const presentation=evidenceValuePresentation(item);if(presentation.tone==='missing')return;
    const field=String(item.fieldPath||'').toLowerCase(),categorical=/(?:^|[._])(pred|prediction)(?:$|[._])/.test(field),priority=categorical?3:/(score|phred|^ds_)/.test(field)&&!field.includes('rank')?2:1,current=families.get(family.key),candidate={...family,presentation,item,priority};
    if(!current||family.key==='spliceai'&&toneRank[presentation.tone]>toneRank[current.presentation.tone]||family.key!=='spliceai'&&priority>current.priority)families.set(family.key,candidate)
  });
  const predictions=[...families.values()];if(!predictions.length)return'<div class="prediction-summary"><dt>Prediction summary</dt><dd class="tone-missing">Not reported</dd></div>';
  const groups=[
    {key:'adverse',label:'damaging',title:'Damaging'},
    {key:'caution',label:'uncertain',title:'Uncertain'},
    {key:'reassuring',label:'benign',title:'Benign or tolerated'},
    {key:'neutral',label:'no direction',title:'No clear direction'}
  ].map(group=>({...group,items:predictions.filter(item=>item.presentation.tone===group.key||group.key==='neutral'&&item.presentation.tone==='informative')})).filter(group=>group.items.length);
  const details=groups.flatMap(group=>group.items.map(item=>`${item.label}: ${item.presentation.display}`)).join(' · '),label=groups.map(group=>`${group.items.length} ${group.label}`).join(', ');
  return`<div class="prediction-summary" title="${escapeHtml(`${details} · Uses categorical predictor calls when available; otherwise uses the displayed score interpretation.`)}"><dt>Prediction summary</dt><dd><span class="prediction-summary-bar" role="img" aria-label="${escapeHtml(`${predictions.length} predictors: ${label}`)}">${groups.map(group=>`<span class="prediction-summary-segment tone-${group.key}" style="flex-grow:${group.items.length}" title="${escapeHtml(`${group.title}: ${group.items.map(item=>item.label).join(', ')}`)}"><b>${group.items.length}</b></span>`).join('')}</span></dd></div>`
}
function numericEvidenceValues(value){
  const decoded=decodeEvidenceValue(value),values=Array.isArray(decoded)?decoded:typeof decoded==='string'?decoded.split(';'):[decoded];
  return values.map(value=>Number(String(value).trim())).filter(value=>Number.isFinite(value))
}
function spliceAiMaximumDeltaItem(items){
  const components=items.filter(item=>fieldSourceIs(item,'spliceai')&&['dsag','dsal','dsdg','dsdl'].includes(evidenceFieldLeaf(item).toLowerCase().replace(/[^a-z0-9]/g,''))),scores=components.flatMap(item=>numericEvidenceValues(item.value)).filter(value=>value>=0&&value<=1);
  if(!scores.length)return null;
  const source=components[0];
  return{...source,fieldPath:'maxDeltaScore',value:String(Math.max(...scores)),scope:'allele',scopeLabel:'Variant-level',synthetic:true}
}
function evidenceScopeLabel(item){if(item.scopeLabel)return item.scopeLabel==='Transcript'?'Selected transcript':item.scopeLabel;return item.scope==='transcript'?'Selected transcript':'Variant-level'}
function clinicalListValues(item,value){
  const field=String(item.fieldPath||'').toLowerCase();
  if(!/condition|phenotype|disease/.test(field))return[];
  const decoded=decodeEvidenceValue(value),values=Array.isArray(decoded)?decoded:typeof decoded==='string'&&decoded.includes('|')?decoded.split('|'):[];
  return[...new Set(values.map(item=>readableTerm(displayDetailValue(item))).filter(item=>item&&item!=='.'&&item!=='-'))]
}
function annotationRow(item,value=item.value){
  const presentation=evidenceFieldPresentation(item),interpreted=evidenceValuePresentation(item,value),resolution=(item.resolution||item.unmatchedTranscript)?{kind:item.resolution?.kind||'ambiguous'}:null,tooltip=evidenceValueTooltip(item,value,{includeSource:true,resolution}),listValues=clinicalListValues(item,value),renderedValue=listValues.length>1?`<ul class="annotation-value-list">${listValues.map(item=>`<li>${escapeHtml(item)}</li>`).join('')}</ul>`:`<b>${escapeHtml(interpreted.display)}</b>`;
  return`<div class="annotation-row fui-key-value-row tone-${interpreted.tone}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(presentation.label)}</strong></span>${renderedValue}</div>`
}
function detailEvidenceSubgroup(title,items,empty='None reported',options={}){if(options.kind==='population')return populationEvidenceSubgroup(items,options.preferredSourceId,empty);return`<div class="key-evidence-subgroup" data-evidence-group="${escapeHtml(title.toLowerCase())}"><div class="key-evidence-subheading fui-section-heading"><strong>${escapeHtml(title)}</strong></div><div class="annotation-list fui-key-value-list">${items.length?items.map(item=>annotationRow(item)).join(''):`<div class="key-evidence-empty">${escapeHtml(empty)}</div>`}</div></div>`}
function groupedEvidenceSection(title,groups,{open=false,className='',extra='',sectionKey=''}={}){
  const expanded=sectionKey?variantDetailOpenSections.has(sectionKey):open,stateAttribute=sectionKey?` data-variant-detail-section="${escapeHtml(sectionKey)}"`:'';
  return`<section class="detail-section annotation-section evidence-domain-section fui-accordion-stack ${escapeHtml(className)}"><details class="evidence-domain collapsible-detail fui-accordion"${stateAttribute} ${expanded?'open':''}><summary><strong>${escapeHtml(title)}</strong></summary><div class="key-evidence-subgroups">${groups.map(([groupTitle,items,empty,options])=>detailEvidenceSubgroup(groupTitle,items,empty,options)).join('')}${extra}</div></details></section>`
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
  const presentation=evidenceFieldPresentation(item),interpreted=evidenceValuePresentation(item);
  return[labelOverride||presentation.label,interpreted.display,evidenceValueTooltip(item,item.value,{includeSource:true}),interpreted.tone]
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
  const normalizeTranscript=value=>String(value||'').trim().split('.')[0],representative=normalizeTranscript(representativeTranscriptId);
  const ranked=items.map((item,index)=>{
    const type=String(consequenceValue(item,'feature_type')||'transcript').toLowerCase(),mane=consequenceValue(item,'mane_select','MANE_SELECT','MANE'),manePlus=consequenceValue(item,'mane_plus_clinical','MANE_PLUS_CLINICAL'),canonical=consequenceValue(item,'canonical','CANONICAL'),isCanonical=canonical===true||canonical===1||['YES','Y','TRUE','1'].includes(String(canonical).toUpperCase()),isRepresentative=representative&&normalizeTranscript(consequenceValue(item,'transcript_id','Feature'))===representative,rank=type==='transcript'?(mane?0:manePlus?1:isRepresentative?2:isCanonical?3:4):5;
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
    ['Gene',consequenceValue(item,'gene_symbol','SYMBOL'),'Gene symbol associated with the selected transcript.'],
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
    ['Gene',consequenceValue(item,'gene_symbol','SYMBOL'),'Gene associated with this feature when VEP reports one.'],
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
  if(label==='Impact')return/high/.test(lower)?'adverse':/moderate/.test(lower)?'caution':/low/.test(lower)?'informative':'neutral';
  if(label==='Gene'||label==='Consequence')return'neutral';
  if(label==='Zygosity')return/reference/.test(lower)?'neutral':'informative';
  if(label==='GQ'){const number=Number(text);return!Number.isFinite(number)?'neutral':number>=30?'reassuring':number<20?'caution':'neutral'}
  if(label==='Overall AF'||label==='Group-max AF')return alleleFrequencyTone(text);
  if(label==='QUAL'){const number=Number(text);return!Number.isFinite(number)?'neutral':number>=30?'reassuring':number<20?'caution':'neutral'}
  if(label==='VCF FILTER')return/^pass$/i.test(text)?'reassuring':text==='.'?'neutral':'caution';
  return'neutral'
}
function variantSummaryCell(label,value,tone,tooltip){
  const resolvedTone=tone||variantFactTone(label,value);
  return`<div class="fui-description-grid__item" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="tone-${escapeHtml(resolvedTone)}">${escapeHtml(displayDetailValue(value))}</dd></div>`
}
function populationFrequencySummary(overall,groupMaximum,overallTooltip,groupMaximumTooltip){
  const tooltip=[`Overall AF: ${overallTooltip}`,`Group-max AF: ${groupMaximumTooltip}`].join(' · ');
  return`<div class="population-af-summary fui-description-grid__item" title="${escapeHtml(tooltip)}"><dt>Population AF</dt><dd><span class="population-af-value tone-${variantFactTone('Overall AF',overall)}">${escapeHtml(displayDetailValue(overall))}</span><small>Group max: <span class="population-af-group-value tone-${variantFactTone('Group-max AF',groupMaximum)}">${escapeHtml(displayDetailValue(groupMaximum))}</span></small></dd></div>`
}
function variantSummaryRow(cells){return`<div class="detail-summary-row fui-description-grid__row" style="--detail-summary-columns:${cells.length}">${cells.join('')}</div>`}

function renderVariantDetail(row,detail){
  rememberVariantDetailSectionState();
  const consequences=detail.consequences||[],evidence=detail.evidence||[],variant=detail.variant||{},unique=[...new Map(consequences.map((item,index)=>[consequenceContextKey(item,index),item])).values()],preferred=preferredConsequence(unique,variant.transcriptId||row.transcriptId),stored=detailConsequenceSelections.get(detail.alleleId||row.alleleId),selected=unique.find((item,index)=>consequenceContextKey(item,index)===stored)||preferred||unique[0]||{},selectedContext=unique.length?consequenceContextKey(selected,unique.indexOf(selected)):'',selectedFeatureType=String(consequenceValue(selected,'feature_type')||'transcript').toLowerCase(),isTranscript=selectedFeatureType==='transcript',transcriptId=isTranscript?String(consequenceValue(selected,'transcript_id','Feature')||''):'',gene=consequenceValue(selected,'gene_symbol','SYMBOL')||variant.geneSymbol||row.geneSymbol||row.gene||'',links=usefulVariantLinks(row,gene,selected,variant);
  const selectedTerms=consequenceValue(selected,'consequence_terms','Consequence'),selectedConsequenceId=selected._annocatConsequenceId,scopedEvidence=evidence.filter(item=>item.scope==='allele'||!selectedConsequenceId||!item.consequenceId||item.consequenceId===selectedConsequenceId),transcriptMetadataFields=new Set(['APPRIS','GENCODE_basic','TSL','Uniprot_acc','aaref','aaalt','aapos']),alignedEvidence=isTranscript?selectedTranscriptEvidence(scopedEvidence,transcriptId):scopedEvidence,metadata=isTranscript?alignedEvidence.filter(item=>String(item.sourceId).toLowerCase()==='dbnsfp'&&transcriptMetadataFields.has(item.fieldPath)):[],combined=alignedEvidence.filter(item=>!dbnsfpTranscriptMetadata.has(item.fieldPath)).map(item=>({...item,consequenceTerms:selectedTerms})),frequency=primaryAlleleFrequency(combined),frequencySourceItems=frequency?combined.filter(item=>String(item.sourceId||'').toLowerCase()===String(frequency.sourceId||'').toLowerCase()):combined,groupMaximum=groupMaximumAlleleFrequency(frequencySourceItems),frequencyDisplay=frequency?evidenceValuePresentation(frequency).display:reportHasFrequencyFields()?'Not reported':'Not available',groupMaximumDisplay=groupMaximum?evidenceValuePresentation(groupMaximum).display:reportHasFrequencyFields()?'Not reported':'Not available',domainGroups=combined.reduce((groups,item)=>{const domain=evidenceDomain(item);(groups[domain]??=[]).push(item);return groups},{}),sampleFacts=sampleOverview(variant);
  const phyloPItem=(domainGroups.conservation||[]).find(item=>String(item.sourceId||'').toLowerCase().includes('phylop')||String(item.fieldPath||'').toLowerCase().includes('phylop')),phyloPPresentation=phyloPItem?evidenceValuePresentation(phyloPItem):{display:'Not reported',tone:'missing'},phyloPTooltip=phyloPItem?evidenceValueTooltip(phyloPItem,phyloPItem.value,{includeSource:true}):'No phyloP conservation score was reported for this position.';
  const clinvarItem=(domainGroups.clinical||[]).find(item=>/(^|[._])(significance|clnsig)([._]|$)|clinical.?significance/i.test(String(item.fieldPath||''))),clinvarPresentation=clinvarItem?evidenceValuePresentation(clinvarItem):{display:'Not reported',tone:'missing'},clinvarTooltip=clinvarItem?evidenceValueTooltip(clinvarItem,clinvarItem.value,{includeSource:true}):'No ClinVar classification was reported for this variant.';
  const revelItem=(domainGroups.prediction||[]).filter(item=>String(item.sourceId||'').toLowerCase().includes('revel')||String(item.fieldPath||'').toLowerCase().includes('revel')).sort((left,right)=>{const rank=item=>{const field=String(item.fieldPath||'').toLowerCase();if(field==='score'||field==='revel_score')return 0;if(field.includes('score')&&!field.includes('rank'))return 1;return 10};return rank(left)-rank(right)})[0]||null,revelPresentation=revelItem?evidenceValuePresentation(revelItem):{display:'Not reported',tone:'missing'},revelTooltip=revelItem?evidenceValueTooltip(revelItem,revelItem.value,{includeSource:true}):'No REVEL score was reported for this variant.';
  const readCounts=sampleFacts.referenceReads==='Not available'&&sampleFacts.alternateReads==='Not available'?'Not available':`${sampleFacts.referenceReads} / ${sampleFacts.alternateReads}`,genotypeNotCalled=sampleFacts.zygosity==='Not called',genotypePhase=genotypeNotCalled&&sampleFacts.genotype!=='Not available'?sampleFacts.genotype:genotypeNotCalled?'Not called':sampleFacts.genotype==='Not available'&&sampleFacts.phase==='Not available'?'Not available':[sampleFacts.genotype,sampleFacts.phase].filter(value=>value!=='Not available').join(' · '),genotypePhaseTooltip=genotypeNotCalled?`Raw sample GT is ${sampleFacts.genotype}; the genotype is missing, so phase does not apply.`:`Raw sample GT and phase for ${sampleFacts.sampleName||'the sample'}. Allele numbers refer to the original VCF ALT order; this row represents ALT ${variant.altIndex}.`,alleleBalanceNumber=Number(String(sampleFacts.alleleBalance).replace('%','')),alleleBalanceTone=!Number.isFinite(alleleBalanceNumber)?variantFactTone('Allele balance',sampleFacts.alleleBalance):/heterozygous/i.test(sampleFacts.zygosity)?alleleBalanceNumber>=30&&alleleBalanceNumber<=70?'reassuring':'caution':/homozygous alternate/i.test(sampleFacts.zygosity)?alleleBalanceNumber>=90?'reassuring':'caution':'neutral',frequencyTooltip=frequency?evidenceValueTooltip(frequency,frequency.value,{includeSource:true}):'No overall population allele frequency was available.',groupMaximumTooltip=groupMaximum?evidenceValueTooltip(groupMaximum,groupMaximum.value,{includeSource:true}):'No population group-maximum allele frequency was available.';
  const options=unique.map((item,index)=>{const id=consequenceContextKey(item,index),featureId=consequenceValue(item,'feature_id','transcript_id','Feature','regulatory_feature_id','motif_feature_id'),featureType=readableTerm(consequenceValue(item,'feature_type')||'transcript'),label=[featureId||featureType,readableTerm(primaryConsequence(consequenceValue(item,'consequence_terms','Consequence'))),consequenceValue(item,'mane_select','MANE_SELECT','MANE')?'MANE Select':consequenceValue(item,'mane_plus_clinical','MANE_PLUS_CLINICAL')?'MANE Plus Clinical':consequenceValue(item,'canonical','CANONICAL')?'Canonical':''].filter(Boolean).join(' · ');return`<option value="${escapeHtml(id)}" ${id===selectedContext?'selected':''}>${escapeHtml(label)}</option>`}).join('');
  $('#variant-detail-title').textContent=`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`;
  const clinicalPriority=item=>{const field=String(item.fieldPath||'').toLowerCase();if(/significance|clnsig/.test(field))return 0;if(/review/.test(field))return 1;if(/condition|phenotype|disease/.test(field))return 2;if(/variant.?class/.test(field))return 3;if(/so.?accession|sequence.?ontology/.test(field))return 4;return 10},clinicalItems=[...(domainGroups.clinical||[])].sort((left,right)=>clinicalPriority(left)-clinicalPriority(right)),predictionItems=domainGroups.prediction||[],rawSplicingItems=domainGroups.splicing||[],spliceAiMaximum=spliceAiMaximumDeltaItem(rawSplicingItems),splicingItems=spliceAiMaximum?[spliceAiMaximum,...rawSplicingItems]:rawSplicingItems,conservationItems=domainGroups.conservation||[],populationItems=sortPopulationEvidence(domainGroups.population||[]),predictionSummary=predictionSummaryBar([...predictionItems,...splicingItems]),technicalGroups=evidenceDomainOrder.filter(domain=>!['clinical','population','prediction','splicing','conservation'].includes(domain)&&domainGroups[domain]?.length).map(domain=>[evidenceDomainTitles[domain],domainGroups[domain]]),provenance=`<div class="detail-provenance"><dl><div><dt>Assembly</dt><dd>GRCh38</dd></div><div><dt>Allele ID</dt><dd>${escapeHtml(detail.alleleId||row.alleleId)}</dd></div><div><dt>Sources</dt><dd>${escapeHtml([...new Set(evidence.map(item=>resourceTitle(item.sourceId)).filter(Boolean))].join(', ')||'Core annotation')}</dd></div><div><dt>Schema</dt><dd>${escapeHtml(displayDetailValue(detail.schemaVersion))}</dd></div></dl>${detail.evidenceTruncated?'<p class="detail-warning">Only the first 5,000 evidence fields are available.</p>':''}</div>`;
  const featureLabel=isTranscript?'selected transcript':`selected ${readableTerm(selectedFeatureType).toLowerCase()} feature`,sectionHeading=isTranscript?'Transcript & molecular effect':`${readableTerm(selectedFeatureType)} effect`,primarySelectedTerm=primaryConsequence(selectedTerms),secondarySelectedTerms=additionalConsequences(selectedTerms),consequenceDisplay=readableTerm(primarySelectedTerm)||readableTerm(row.consequence||variant.consequence),selectedImpact=readableTerm(consequenceValue(selected,'impact','IMPACT')||row.impact||variant.impact),consequenceTone=selectedImpact?variantFactTone('Impact',selectedImpact):'neutral',depthQuality=`${sampleFacts.depth} / ${sampleFacts.quality}`,allelicSupport=`${readCounts} · ${sampleFacts.alleleBalance}`,vcfCall=`${displayDetailValue(variant.filter)} · QUAL ${displayDetailValue(variant.quality)}`,summaryRows=[
    variantSummaryRow([
      variantSummaryCell('Gene',gene,'neutral',`Gene symbol for the ${featureLabel} consequence when reported.`),
      variantSummaryCell('Consequence',consequenceDisplay,consequenceTone,`Primary Sequence Ontology consequence for the ${featureLabel}.${secondarySelectedTerms.length?` Additional terms for this same feature: ${secondarySelectedTerms.map(readableTerm).join(', ')}.`:''}${selectedImpact?` VEP impact: ${selectedImpact}.`:''}`),
      variantSummaryCell('Zygosity',sampleFacts.zygosity,null,`Relationship of ${sampleFacts.sampleName||'the sample'} to this row's selected ALT allele, derived from GT and alt_index ${variant.altIndex}.`),
      variantSummaryCell('ClinVar',clinvarPresentation.display,clinvarPresentation.tone,clinvarTooltip)
    ]),
    variantSummaryRow([
      populationFrequencySummary(frequencyDisplay,groupMaximumDisplay,frequencyTooltip,groupMaximumTooltip),
      variantSummaryCell('Conservation',phyloPPresentation.display,phyloPPresentation.tone,phyloPTooltip),
      variantSummaryCell('REVEL',revelPresentation.display,revelPresentation.tone,revelTooltip),
      predictionSummary
    ]),
    variantSummaryRow([
      variantSummaryCell('Genotype / phase',genotypePhase,null,genotypePhaseTooltip),
      variantSummaryCell('Depth / GQ',depthQuality,variantFactTone('GQ',sampleFacts.quality),'Total read depth from FORMAT/DP followed by phred-scaled genotype quality from FORMAT/GQ.'),
      variantSummaryCell('Allelic support',allelicSupport,alleleBalanceTone,`Reference / selected-ALT read depths from FORMAT/AD followed by the selected ALT fraction of all allele depths. Other ALT depths are not combined with this row.`),
      variantSummaryCell('VCF call',vcfCall,variantFactTone('VCF FILTER',variant.filter),'VCF FILTER status followed by the phred-scaled variant confidence from QUAL.')
    ])
  ].join('');
  $('#variant-detail-body').innerHTML=`<section class="detail-overview"><div class="detail-links">${links.map(([label,url,title])=>`<a class="fui-button fui-button--small" href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtml(title)}">${escapeHtml(label)} ↗</a>`).join('')}</div><dl class="detail-summary fui-description-grid">${summaryRows}</dl></section>${groupedEvidenceSection('Clinical & population evidence',[['Clinical',clinicalItems],['Population',populationItems,'None reported',{kind:'population',preferredSourceId:frequency?.sourceId}]],{className:'clinical-population-section',sectionKey:'clinical-population'})}<section class="detail-section transcript-context fui-accordion-stack"><details class="transcript-details collapsible-detail fui-accordion" data-variant-detail-section="transcript-molecular" ${variantDetailOpenSections.has('transcript-molecular')?'open':''}><summary><strong>${escapeHtml(sectionHeading)}</strong></summary><div class="transcript-context-body">${unique.length?`<select id="detail-consequence-select" class="fui-select" aria-label="Consequence context" title="The selected feature controls the effect and identifier fields below.">${options}</select><div class="selected-transcript-card">${isTranscript?transcriptDetail(selected,metadata):nonTranscriptDetail(selected)}</div>`:'<p class="detail-empty">No molecular consequence was recorded.</p>'}${detail.consequencesTruncated?'<p class="detail-warning">Only the first 1,000 consequences are available.</p>':''}</div></details></section>${groupedEvidenceSection('Predictions & conservation',[['Prediction scores',predictionItems],['Splicing',splicingItems],['Conservation',conservationItems]],{className:'prediction-evidence-section',sectionKey:'predictions-conservation'})}${groupedEvidenceSection('Technical details & provenance',technicalGroups,{className:'technical-provenance-section',extra:provenance,sectionKey:'technical-provenance'})}`;
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
function tableValuePresentation(key,value,resolution=null){
  if(key==='impact'){const display=readableTerm(value),tone=/high/i.test(value)?'adverse':/moderate/i.test(value)?'caution':/low/i.test(value)?'informative':'neutral';return{display,tone,description:'Predicted impact category from fastVEP; HIGH is most severe, followed by MODERATE, LOW, and MODIFIER.'}}
  if(key==='consequence'||key==='biotype')return{display:readableTerm(value),tone:'neutral',description:'Human-readable Sequence Ontology annotation.'};
  if(key.startsWith('evidence:')){const field=resultFieldCatalog[Number(key.slice(9))]||{},interpreted=evidenceValuePresentation(field,value);return{...interpreted,description:evidenceValueTooltip(field,value,{includeSource:true,resolution})}}
  return{display:displayDetailValue(value),tone:'neutral',description:coreColumnPresentation(key,key).description}
}
function enhanceResultGrid(){
  const table=document.querySelector('.table-wrap table');if(!table)return;
  const shown=displayColumns(),keys=['selection','candidate',...shown.map(([key])=>key)],tableWrap=table.parentElement,setTableWidth=()=>{const width=`${keys.reduce((total,key)=>total+gridWidth(key),0)}px`;table.style.width=width;tableWrap.style.setProperty('--result-table-width',width)};let colgroup=table.querySelector('colgroup');if(!colgroup){colgroup=document.createElement('colgroup');table.insertBefore(colgroup,table.firstChild)}colgroup.innerHTML=keys.map(key=>`<col data-grid-column="${escapeHtml(key)}" style="width:${gridWidth(key)}px">`).join('');setTableWidth();
  [...$('#head').children].forEach((header,index)=>{const key=keys[index];header.dataset.gridColumn=key;if(index<2)return;header.draggable=true;header.classList.add('draggable-column');header.addEventListener('dragstart',event=>{if(event.target.closest('.column-resizer')){event.preventDefault();return}header.dataset.columnDragged='true';event.dataTransfer.effectAllowed='move';event.dataTransfer.setData('text/plain',key);header.classList.add('column-dragging')});header.addEventListener('dragover',event=>{event.preventDefault();event.dataTransfer.dropEffect='move';header.classList.add('column-drag-target')});header.addEventListener('dragleave',()=>header.classList.remove('column-drag-target'));header.addEventListener('drop',event=>{event.preventDefault();header.classList.remove('column-drag-target');moveResultColumn(event.dataTransfer.getData('text/plain'),key)});header.addEventListener('dragend',()=>{header.classList.remove('column-dragging');document.querySelectorAll('.column-drag-target').forEach(item=>item.classList.remove('column-drag-target'));setTimeout(()=>delete header.dataset.columnDragged,0)});header.addEventListener('click',event=>{if(header.dataset.columnDragged==='true'){event.preventDefault();event.stopImmediatePropagation()}},true);if(!header.querySelector('.column-resizer')){const custom=resultGridWidths.has(gridWidthStorageKey(key)),title=custom?'Drag to resize; double-click to restore the default width':'Drag to resize; double-click to fit loaded values';header.insertAdjacentHTML('beforeend',`<span class="column-resizer" data-resize-column="${escapeHtml(key)}" title="${title}"></span>`)}});
  $('#head').querySelectorAll('[data-resize-column]').forEach(handle=>{handle.addEventListener('pointerdown',event=>{event.preventDefault();event.stopPropagation();const key=handle.dataset.resizeColumn,storageKey=gridWidthStorageKey(key),startX=event.clientX,startWidth=gridWidth(key),col=colgroup.querySelector(`[data-grid-column="${CSS.escape(key)}"]`),move=moveEvent=>{const width=Math.max(minimumGridWidth(key),Math.min(520,startWidth+moveEvent.clientX-startX));resultGridWidths.set(storageKey,String(width));col.style.width=`${width}px`;setTableWidth()},up=()=>{window.removeEventListener('pointermove',move);window.removeEventListener('pointerup',up);handle.title='Drag to resize; double-click to restore the default width';localStorage.setItem('annocat.resultColumnWidths',JSON.stringify(Object.fromEntries(resultGridWidths)))};window.addEventListener('pointermove',move);window.addEventListener('pointerup',up)});handle.addEventListener('dblclick',event=>{event.preventDefault();event.stopPropagation();const key=handle.dataset.resizeColumn,storageKey=gridWidthStorageKey(key),col=colgroup.querySelector(`[data-grid-column="${CSS.escape(key)}"]`);if(resultGridWidths.has(storageKey))resultGridWidths.delete(storageKey);else resultGridWidths.set(storageKey,String(fittedGridWidth(key,handle.parentElement,table)));col.style.width=`${gridWidth(key)}px`;setTableWidth();handle.title=resultGridWidths.has(storageKey)?'Drag to resize; double-click to restore the default width':'Drag to resize; double-click to fit loaded values';localStorage.setItem('annocat.resultColumnWidths',JSON.stringify(Object.fromEntries(resultGridWidths)))})});
  $('#rows').querySelectorAll('tr[data-allele-id]').forEach(element=>{const row=variants.find(item=>item.alleleId===element.dataset.alleleId);if(!row)return;shown.forEach(([key],index)=>{const cell=element.children[index+2],value=resultColumnRawValue(row,key);if(!cell)return;cell.classList.add('result-cell',`result-cell-${String(key).replace(/[^a-z0-9_-]/gi,'-')}`);if(key.startsWith('evidence:')){const evidenceIndex=Number(key.slice(9)),presentation=tableValuePresentation(key,value,row.evidenceResolution?.[evidenceIndex]);cell.innerHTML=`<span class="table-value tone-${presentation.tone}" title="${escapeHtml(presentation.description)}">${escapeHtml(presentation.display)}</span>`}else if(key==='consequence'||key==='biotype')cell.textContent=readableTerm(value)})});
}

function renderTable(event){renderTableBase(event);enhanceResultGrid()}

  return{
    evidenceFieldPresentation(...args){syncState();return evidenceFieldPresentation(...args)},
    consequenceTerms(...args){syncState();return consequenceTerms(...args)},
    evidenceValuePresentation(...args){syncState();return evidenceValuePresentation(...args)},
    renderVariantDetail(...args){syncState();return renderVariantDetail(...args)},
    renderTable(...args){syncState();return renderTable(...args)}
  }
}
