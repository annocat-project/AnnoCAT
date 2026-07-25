import { openFluentDialog } from './ui-components.js';

export function createPhenotypeFeature({$,escapeHtml,prototypeIcon,showPage}){
  let currentResultRun=null,resourceStates={};
  let phenotypeProfile=null,phenotypeExploration=null,phenotypeDialogRunId=null,phenotypeSampleName='',phenotypeSearchTimer=null,phenotypeResultLimit=100,phenotypeSearchResults=[],phenotypeSearchActiveIndex=-1,phenotypeResultSort='phenotype',phenotypeMessage='',phenotypeOnlineConsent=false,phenotypeSaveRevision=0,phenotypeSaveChain=Promise.resolve();
  const phenotypeSampleSelections=new Map();

function ensurePhenotypeDialog(){
  let dialog=$('#phenotype-dialog');
  if(dialog)return dialog;
  document.body.insertAdjacentHTML('beforeend',`<dialog id="phenotype-dialog" class="phenotype-dialog fui-dialog" aria-labelledby="phenotype-dialog-title"><div class="phenotype-dialog-heading fui-dialog__header"><div><p class="kicker">Experimental phenotype prioritization</p><h2 id="phenotype-dialog-title">Phenotype prioritization</h2><p class="fui-dialog__description">Compare patient findings with HPO disease profiles and review report evidence in one candidate list.</p></div><button type="button" class="fui-button fui-button--icon" data-close-phenotypes aria-label="Close phenotype prioritization">${prototypeIcon('close')}</button></div><div class="phenotype-dialog-body fui-dialog__content" data-phenotype-body><p class="phenotype-loading fui-text--muted">Loading phenotype profile...</p></div></dialog>`);
  dialog=$('#phenotype-dialog');
  dialog.addEventListener('click',async event=>{
    if(event.target===dialog){dialog.close();return}
    if(event.target.closest('[data-close-phenotypes]')){dialog.close();return}
    const result=event.target.closest('[data-phenotype-search-result]');
    if(result){
      const term=phenotypeSearchResults.find(item=>item.id===result.dataset.phenotypeSearchResult);
      if(term)await addPhenotypeTerm(term,$('#phenotype-presence')?.value||'observed');
      return
    }
    const remove=event.target.closest('[data-remove-phenotype]');
    if(remove){await removePhenotypeTerm(remove.dataset.phenotypeKind,remove.dataset.removePhenotype);return}
    if(event.target.closest('[data-rank-phenotypes]')){await rankPhenotypes();return}
    if(event.target.closest('[data-explore-report-phenotypes]')){await exploreReportPhenotypes();return}
    if(event.target.closest('[data-clear-phenotypes]')){await clearPhenotypes();return}
    if(event.target.closest('[data-install-hpo]')){dialog.close();showPage('resources');return}
    if(event.target.closest('[data-more-phenotype-results]')){phenotypeResultLimit+=100;renderPhenotypeResults();return}
  });
  dialog.addEventListener('input',event=>{
    if(!event.target.matches('[data-phenotype-search]'))return;
    clearTimeout(phenotypeSearchTimer);
    const query=event.target.value.trim();
    phenotypeSearchActiveIndex=-1;
    if(query.length<2){phenotypeSearchResults=[];renderPhenotypeSearchResults();return}
    phenotypeSearchTimer=setTimeout(()=>searchPhenotypeTerms(query),180)
  });
  dialog.addEventListener('keydown',async event=>{
    const input=event.target.closest('[data-phenotype-search]');
    if(!input)return;
    if(event.key==='Escape'&&phenotypeSearchResults.length){event.preventDefault();phenotypeSearchResults=[];phenotypeSearchActiveIndex=-1;renderPhenotypeSearchResults();return}
    if(!['ArrowDown','ArrowUp','Enter'].includes(event.key)||!phenotypeSearchResults.length)return;
    event.preventDefault();
    if(event.key==='ArrowDown')phenotypeSearchActiveIndex=Math.min(phenotypeSearchResults.length-1,phenotypeSearchActiveIndex+1);
    if(event.key==='ArrowUp')phenotypeSearchActiveIndex=Math.max(0,phenotypeSearchActiveIndex<0?phenotypeSearchResults.length-1:phenotypeSearchActiveIndex-1);
    if(event.key==='Enter'){
      const term=phenotypeSearchResults[Math.max(0,phenotypeSearchActiveIndex)];
      if(term)await addPhenotypeTerm(term,$('#phenotype-presence')?.value||'observed');
      return
    }
    updatePhenotypeSearchActiveOption()
  });
  dialog.addEventListener('change',event=>{
    if(event.target.matches('[data-phenotype-sample]')){
      phenotypeSampleName=event.target.value;
      if(phenotypeDialogRunId)phenotypeSampleSelections.set(phenotypeDialogRunId,phenotypeSampleName);
      phenotypeExploration=null;
      if(phenotypeProfile)phenotypeProfile.ranking=null;
      phenotypeMessage=phenotypeSampleName?'Patient sample changed. Run a comparison to calculate carried-ALT overlap.':'Choose a patient sample to calculate report overlap.';
      renderPhenotypeDialog();
      return
    }
    if(event.target.matches('[data-phenotype-sort]')){
      phenotypeResultSort=event.target.value;
      phenotypeResultLimit=100;
      renderPhenotypeResults()
    }
    if(event.target.matches('[data-monarch-enrichment]'))phenotypeOnlineConsent=event.target.checked
  });
  return dialog
}
function phenotypeTermChips(kind,terms){
  if(!terms.length)return'<p class="phenotype-empty">None specified</p>';
  return`<div class="phenotype-chips">${terms.map(term=>`<span><b>${escapeHtml(term.label)}</b><small>${escapeHtml(term.id)}</small><button type="button" class="fui-button fui-button--small fui-button--icon fui-button--subtle" data-phenotype-kind="${kind}" data-remove-phenotype="${escapeHtml(term.id)}" aria-label="Remove ${escapeHtml(term.label)}">${prototypeIcon('close')}</button></span>`).join('')}</div>`
}
function renderPhenotypeDialog(){
  const body=$('#phenotype-dialog [data-phenotype-body]');
  if(!body)return;
  if(!phenotypeProfile){body.innerHTML='<p class="phenotype-loading">Loading phenotype profile...</p>';return}
  if(resourceStates.hpo&&!resourceStates.hpo.ready){
    const status=resourceStates.hpo.label||'Not installed';
    body.innerHTML=`<section class="phenotype-unavailable"><div class="phenotype-info-icon">${prototypeIcon('info')}</div><h3>Install Human Phenotype Ontology data</h3><p>Local term search and phenotype similarity require the managed HPO release. The ontology, disease annotations, and disease-gene associations install together as one data source.</p><p><strong>Status:</strong> ${escapeHtml(status)}</p><button type="button" class="primary fui-button fui-button--primary" data-install-hpo>Open Data sources</button></section>`;
    return
  }
  const sampleNames=phenotypeProfile.sampleNames||[],release=phenotypeProfile.hpoRelease||'installed release',observed=phenotypeProfile.observed||[],excluded=phenotypeProfile.excluded||[],hasObserved=observed.length>0;
  const sampleControl=sampleNames.length>1?`<label class="phenotype-sample-picker"><span>Patient sample</span><select class="fui-select" data-phenotype-sample><option value="">Choose a sample</option>${sampleNames.map(name=>`<option value="${escapeHtml(name)}" ${name===phenotypeSampleName?'selected':''}>${escapeHtml(name)}</option>`).join('')}</select><small>Only exact ALT alleles carried by this sample contribute to report overlap.</small></label>`:sampleNames.length===1?`<div class="phenotype-sample-picker fixed"><span>Patient sample</span><strong>${escapeHtml(sampleNames[0])}</strong><small>Exact carried ALT alleles are used for report overlap.</small></div>`:`<div class="phenotype-sample-picker unavailable"><span>Report overlap unavailable</span><small>This report has no sample genotype columns. Phenotype similarity can still be calculated.</small></div>`;
  body.innerHTML=`${sampleControl}<section class="phenotype-profile-editor" aria-label="Patient phenotype profile"><div class="phenotype-profile-column fui-card"><h3>Observed findings</h3><p>Phenotypic abnormalities present in the patient.</p>${phenotypeTermChips('observed',observed)}</div><div class="phenotype-profile-column fui-card"><h3>Explicitly absent findings</h3><p>Only add abnormalities that were assessed and not found.</p>${phenotypeTermChips('excluded',excluded)}</div></section><section class="phenotype-term-search"><label><span>Add as</span><select class="fui-select" id="phenotype-presence"><option value="observed">Observed</option><option value="excluded">Explicitly absent</option></select></label><label class="phenotype-search-field"><span>HPO phenotypic abnormality</span><input class="fui-input" type="search" data-phenotype-search role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="phenotype-search-results" placeholder="Search by feature, synonym, or HP identifier" autocomplete="off"><div id="phenotype-search-results" class="phenotype-search-results fui-popover fui-popover--listbox" data-phenotype-search-results role="listbox" aria-label="Matching HPO terms"></div></label></section><div class="phenotype-actions"><p class="fui-caption" data-phenotype-message>${escapeHtml(phenotypeMessage)}</p><label class="phenotype-online-option"><input class="fui-checkbox" type="checkbox" data-monarch-enrichment ${phenotypeOnlineConsent?'checked':''}><span>Add Monarch gene suggestions</span><small>Sends observed HPO IDs and returns up to 50 genes.</small></label>${hasObserved?'':`<button type="button" class="fui-button" data-explore-report-phenotypes ${phenotypeSampleName?'':'disabled'}>Explore report associations</button>`}<button type="button" class="fui-button" data-clear-phenotypes>Clear profile</button><button type="button" class="primary fui-button fui-button--primary" data-rank-phenotypes ${hasObserved?'':'disabled'}>Prioritize candidates</button></div><section class="phenotype-ranking" data-phenotype-ranking></section><p class="phenotype-attribution">Uses Human Phenotype Ontology release ${escapeHtml(release)}. <a href="https://human-phenotype-ontology.github.io/license.html" target="_blank" rel="noopener noreferrer">HPO license and attribution</a>.</p>`;
  renderPhenotypeResults()
}
function renderPhenotypeSearchResults(){
  const host=$('#phenotype-dialog [data-phenotype-search-results]'),input=$('#phenotype-dialog [data-phenotype-search]');
  if(!host)return;
  if(phenotypeSearchActiveIndex>=phenotypeSearchResults.length)phenotypeSearchActiveIndex=phenotypeSearchResults.length-1;
  host.innerHTML=phenotypeSearchResults.length?phenotypeSearchResults.map((term,index)=>`<button id="phenotype-search-option-${index}" type="button" role="option" aria-selected="${index===phenotypeSearchActiveIndex}" class="fui-menu-item ${index===phenotypeSearchActiveIndex?'active':''}" data-phenotype-search-result="${escapeHtml(term.id)}"><span><strong>${escapeHtml(term.label)}</strong><small>${escapeHtml(term.id)}</small></span>${term.synonyms?.length?`<em>${escapeHtml(term.synonyms.join('; '))}</em>`:''}</button>`).join(''):'';
  if(input){input.setAttribute('aria-expanded',String(phenotypeSearchResults.length>0));if(phenotypeSearchActiveIndex>=0)input.setAttribute('aria-activedescendant',`phenotype-search-option-${phenotypeSearchActiveIndex}`);else input.removeAttribute('aria-activedescendant')}
}
function updatePhenotypeSearchActiveOption(){
  const input=$('#phenotype-dialog [data-phenotype-search]');
  $('#phenotype-dialog')?.querySelectorAll('[data-phenotype-search-result]').forEach((option,index)=>{const active=index===phenotypeSearchActiveIndex;option.classList.toggle('active',active);option.setAttribute('aria-selected',String(active));if(active)option.scrollIntoView({block:'nearest'})});
  if(input&&phenotypeSearchActiveIndex>=0)input.setAttribute('aria-activedescendant',`phenotype-search-option-${phenotypeSearchActiveIndex}`)
}
async function searchPhenotypeTerms(query){
  const input=$('#phenotype-dialog [data-phenotype-search]');
  try{
    const response=await fetch(`/api/phenotypes/terms?q=${encodeURIComponent(query)}&limit=20`),body=await response.json();
    if(!response.ok)throw new Error(body.error||'HPO term search failed');
    if(input?.value.trim()!==query)return;
    phenotypeSearchResults=body.terms||[];
    phenotypeSearchActiveIndex=-1;
    renderPhenotypeSearchResults()
  }catch(error){phenotypeMessage=error.message;const message=$('#phenotype-dialog [data-phenotype-message]');if(message)message.textContent=phenotypeMessage}
}
async function queuePhenotypeProfileRequest(payload){
  if(!currentResultRun||phenotypeDialogRunId!==currentResultRun.id)return{current:false};
  const runId=currentResultRun.id,revision=++phenotypeSaveRevision,snapshot=JSON.parse(JSON.stringify(payload));
  const request=phenotypeSaveChain.catch(()=>{}).then(async()=>{
    const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/phenotypes`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify(snapshot)}),body=await response.json();
    if(!response.ok)throw new Error(body.error||'Phenotype profile could not be saved');
    return body
  });
  phenotypeSaveChain=request.then(()=>undefined,()=>undefined);
  try{
    const body=await request,current=phenotypeDialogRunId===runId&&currentResultRun?.id===runId&&revision===phenotypeSaveRevision;
    if(current)phenotypeProfile=body;
    return{current,body}
  }catch(error){
    error.phenotypeSaveRevision=revision;
    throw error
  }
}
async function persistPhenotypeProfile(){
  return queuePhenotypeProfileRequest({action:'save',observed:[...(phenotypeProfile.observed||[])],excluded:[...(phenotypeProfile.excluded||[])]})
}
async function addPhenotypeTerm(term,kind){
  const other=kind==='observed'?'excluded':'observed',terms=phenotypeProfile[kind]||[];
  if(!terms.some(item=>item.id===term.id))terms.push({id:term.id,label:term.label});
  phenotypeProfile[kind]=terms;
  phenotypeProfile[other]=(phenotypeProfile[other]||[]).filter(item=>item.id!==term.id);
  phenotypeProfile.ranking=null;
  phenotypeExploration=null;
  phenotypeSearchResults=[];
  phenotypeMessage='Saving profile...';
  renderPhenotypeDialog();
  try{const result=await persistPhenotypeProfile();if(result.current){phenotypeMessage='Profile saved locally.';renderPhenotypeDialog()}}catch(error){if(error.phenotypeSaveRevision===phenotypeSaveRevision){phenotypeMessage=error.message;renderPhenotypeDialog()}}
}
async function removePhenotypeTerm(kind,id){
  phenotypeProfile[kind]=(phenotypeProfile[kind]||[]).filter(term=>term.id!==id);
  phenotypeProfile.ranking=null;
  phenotypeExploration=null;
  phenotypeMessage='Saving profile...';
  renderPhenotypeDialog();
  try{const result=await persistPhenotypeProfile();if(result.current){phenotypeMessage='Profile saved locally.';renderPhenotypeDialog()}}catch(error){if(error.phenotypeSaveRevision===phenotypeSaveRevision){phenotypeMessage=error.message;renderPhenotypeDialog()}}
}
async function clearPhenotypes(){
  if(!currentResultRun)return;
  try{
    const result=await queuePhenotypeProfileRequest({action:'clear'});
    if(result.current){phenotypeExploration=null;phenotypeMessage='';phenotypeResultLimit=100;renderPhenotypeDialog()}
  }catch(error){if(error.phenotypeSaveRevision===phenotypeSaveRevision){phenotypeMessage=error.message;renderPhenotypeDialog()}}
}
async function rankPhenotypes(){
  if(!currentResultRun||(phenotypeProfile.observed||[]).length===0)return;
  await phenotypeSaveChain.catch(()=>{});
  if(!currentResultRun||phenotypeDialogRunId!==currentResultRun.id)return;
  const button=$('#phenotype-dialog [data-rank-phenotypes]');
  if(button){button.disabled=true;button.textContent='Comparing disease profiles...'}
  phenotypeMessage='Computing phenotype-only similarity across the local HPO disease corpus. Exact carried-ALT overlap is shown separately and does not affect similarity order.';
  const message=$('#phenotype-dialog [data-phenotype-message]');if(message)message.textContent=phenotypeMessage;
  try{
    phenotypeOnlineConsent=Boolean($('#phenotype-dialog [data-monarch-enrichment]')?.checked);
    const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/phenotypes/rank`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({observed:phenotypeProfile.observed||[],excluded:phenotypeProfile.excluded||[],onlineConsent:phenotypeOnlineConsent,sampleName:phenotypeSampleName||null})}),body=await response.json();
    if(!response.ok)throw new Error(body.error||'Phenotype ranking failed');
    phenotypeProfile=body;phenotypeExploration=null;phenotypeResultLimit=100;phenotypeMessage=`Evaluated ${Number(body.ranking?.evaluatedDiseases||0).toLocaleString()} disease profiles.`;
    renderPhenotypeDialog()
  }catch(error){phenotypeMessage=error.message;renderPhenotypeDialog()}
}
async function exploreReportPhenotypes(){
  if(!currentResultRun||!phenotypeSampleName)return;
  const button=$('#phenotype-dialog [data-explore-report-phenotypes]');
  if(button){button.disabled=true;button.textContent='Finding associations...'}
  phenotypeMessage=`Finding HPO disease associations whose Mendelian genes overlap exact ALT alleles carried by ${phenotypeSampleName} on PASS rows with a representative HIGH or MODERATE VEP impact.`;
  const message=$('#phenotype-dialog [data-phenotype-message]');if(message)message.textContent=phenotypeMessage;
  try{
    const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/phenotypes/explore`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({sampleName:phenotypeSampleName})}),body=await response.json();
    if(!response.ok)throw new Error(body.error||'Report phenotype associations could not be loaded');
    phenotypeExploration=body;phenotypeResultLimit=100;phenotypeMessage=`Found ${Number(body.associatedDiseases||0).toLocaleString()} HPO disease associations overlapping ${Number(body.reportGeneCount||0).toLocaleString()} genes with carried ALT alleles.`;
    renderPhenotypeDialog()
  }catch(error){phenotypeMessage=error.message;renderPhenotypeDialog()}
}
function phenotypeAssociationLabel(type){
  const normalized=String(type||'').toUpperCase();
  if(normalized==='MENDELIAN')return'Mendelian disease association';
  if(normalized==='POLYGENIC')return'Polygenic contribution';
  return'Association type unspecified'
}
function phenotypeCandidateCard(disease,{reportOnly=false,onlineByGene=new Map(),hasReportSample=true,displayOrder=null,grouped=false}={}){
  const overlap=disease.reportOverlap||{},matches=disease.matchedPhenotypes||[],phenotypes=disease.phenotypes||[],associations=disease.genes||[],overlapping=overlap.genes||[];
  const phenotypeRank=Number(disease.phenotypeRank),rank=Number(reportOnly?disease.order:grouped&&displayOrder!==null?displayOrder:phenotypeRank),rankLabel=reportOnly?'report association':grouped?`candidate · phenotype rank #${phenotypeRank.toLocaleString()}`:'phenotype similarity',geneSymbols=[...new Set(associations.map(gene=>gene.symbol).filter(Boolean))],mendelianCount=new Set(associations.filter(gene=>String(gene.associationType).toUpperCase()==='MENDELIAN').map(gene=>String(gene.symbol).toUpperCase())).size;
  const onlineMatches=geneSymbols.map(symbol=>onlineByGene.get(symbol.toUpperCase())).filter(Boolean).sort((left,right)=>Number(left.rank)-Number(right.rank)),bestOnline=onlineMatches[0];
  const phenotypeValue=reportOnly?`${phenotypes.length}`:`${Number(disease.phenotypeScore).toFixed(1)}`,phenotypeLabel=reportOnly?'HPO findings':'Phenotype match',phenotypeNote=reportOnly?'highlighted disease annotations':`${Number(disease.queryCoverage).toFixed(0)}% direct matches`;
  const reportValue=overlap.hasOverlap?`${overlapping.length}`:hasReportSample?'None':'Not run',reportLabel=overlap.hasOverlap?`Report ${overlapping.length===1?'gene':'genes'}`:'Report evidence',reportNote=overlap.hasOverlap?'Exact carried ALT; representative effect':hasReportSample?'No Mendelian carried-ALT overlap':'No patient sample selected';
  const conflictValue=reportOnly?`${mendelianCount}`:Number(disease.conflictScore)>0?`${Number(disease.conflictScore).toFixed(0)}/100`:'None',conflictLabel=reportOnly?'Mendelian links':'Potential conflicts',conflictNote=reportOnly?'used for report support':Number(disease.conflictScore)>0?'Not used in similarity order':'No contradiction detected';
  const reportRows=overlapping.map(gene=>`<span><b>${escapeHtml(gene.symbol)}</b><small>${escapeHtml(gene.tierLabel)} · ${Number(gene.variantCount).toLocaleString()} carried ALT ${Number(gene.variantCount)===1?'row':'rows'}</small></span>`).join('');
  const matchRows=(reportOnly?phenotypes:matches).map(match=>reportOnly?`<span><b>${escapeHtml(match.label)}</b><small>${escapeHtml(match.id)}</small></span>`:`<span><b>${escapeHtml(match.query.label)}</b><small>${escapeHtml(match.diseaseTerm.label)} · ${Math.round(Number(match.similarity)*100)}/100 semantic match${match.diseaseAnnotation?.frequencyLabel?` · ${escapeHtml(match.diseaseAnnotation.frequencyLabel)}`:''}</small></span>`).join('');
  const associationRows=associations.slice(0,16).map(gene=>{const online=onlineByGene.get(String(gene.symbol||'').toUpperCase());return`<span><b>${escapeHtml(gene.symbol)}</b><small>${escapeHtml(phenotypeAssociationLabel(gene.associationType))}${online?` · Monarch #${Number(online.rank).toLocaleString()}`:''}</small></span>`}).join('');
  return`<article class="phenotype-candidate fui-card ${overlap.hasOverlap?'report-supported':''}"><div class="phenotype-candidate-heading"><div><span class="phenotype-candidate-rank">#${rank.toLocaleString()} ${rankLabel}</span><h4>${escapeHtml(disease.diseaseName)}</h4><p>${escapeHtml(disease.diseaseId)}${geneSymbols.length?` <span aria-hidden="true">·</span> ${geneSymbols.slice(0,6).map(symbol=>escapeHtml(symbol)).join(', ')}${geneSymbols.length>6?` +${geneSymbols.length-6}`:''}`:''}</p></div>${bestOnline?`<span class="phenotype-source-badge fui-badge" title="Optional Monarch phenotype-to-gene result; not combined with the local score">Monarch #${Number(bestOnline.rank).toLocaleString()}</span>`:''}</div><div class="phenotype-evidence-summary"><div><span>${phenotypeLabel}</span><strong>${phenotypeValue}</strong><small>${phenotypeNote}</small></div><div class="${overlap.hasOverlap?'supporting':''}"><span>${reportLabel}</span><strong>${reportValue}</strong><small>${reportNote}</small></div><div class="${!reportOnly&&Number(disease.conflictScore)>0?'conflicting':''}"><span>${conflictLabel}</span><strong>${conflictValue}</strong><small>${conflictNote}</small></div></div><details class="phenotype-candidate-details fui-accordion"><summary>Review evidence</summary><div class="phenotype-candidate-evidence">${matchRows?`<section><h5>${reportOnly?'HPO disease profile':'Phenotype evidence'}</h5>${matchRows}</section>`:''}${reportRows?`<section><h5>Report evidence</h5>${reportRows}<p>Uses exact ALT alleles carried by the selected sample, literal VCF PASS, and the report's representative effect. This is not evidence of pathogenicity or causality.</p></section>`:`<section><h5>Report evidence</h5><p>${hasReportSample?'No carried ALT with a representative HIGH or MODERATE effect overlapped a Mendelian disease gene.':'Choose a patient sample to evaluate exact carried-ALT overlap.'}</p></section>`}${associationRows?`<section><h5>Gene-disease relationships</h5>${associationRows}${associations.length>16?`<p>${associations.length-16} additional associations are not shown.</p>`:''}</section>`:''}${!reportOnly&&Number(disease.conflictScore)>0?`<section><h5>Potential conflicts</h5><p>Observed or explicitly absent findings conflict with this disease profile at ${Number(disease.conflictScore).toFixed(0)}/100 similarity. This signal does not change the phenotype order.${disease.conflictFrequencyComplete?'':' Some HPO disease-feature frequencies were unavailable, so those matches are unweighted.'}</p></section>`:''}</div></details></article>`
}
function comparePhenotypeCandidates(left,right,reportSupportedFirst=false){
  if(reportSupportedFirst){
    const supportOrder=Number(Boolean(right.reportOverlap?.hasOverlap))-Number(Boolean(left.reportOverlap?.hasOverlap));
    if(supportOrder)return supportOrder
  }
  const leftRank=Number.isFinite(Number(left.phenotypeRank))?Number(left.phenotypeRank):Number.MAX_SAFE_INTEGER,rightRank=Number.isFinite(Number(right.phenotypeRank))?Number(right.phenotypeRank):Number.MAX_SAFE_INTEGER;
  return leftRank-rightRank||String(left.diseaseId||'').localeCompare(String(right.diseaseId||''))
}
function phenotypeCandidateList(diseases,options={}){
  const grouped=phenotypeResultSort==='overlap';
  return diseases.map((disease,index)=>{
    const supported=Boolean(disease.reportOverlap?.hasOverlap),previousSupported=index?Boolean(diseases[index-1].reportOverlap?.hasOverlap):null;
    const heading=grouped&&(index===0||supported!==previousSupported)?`<div class="phenotype-candidate-group-heading"><h4>${supported?'Report-supported candidates':'Other phenotype matches'}</h4><p>${supported?'Qualifying report overlap; sorted by phenotype match.':'No qualifying report overlap; sorted by phenotype match.'}</p></div>`:'';
    return`${heading}${phenotypeCandidateCard(disease,{...options,displayOrder:index+1,grouped})}`
  }).join('')
}
function renderPhenotypeResults(){
  const host=$('#phenotype-dialog [data-phenotype-ranking]'),ranking=phenotypeProfile?.ranking;
  if(!host)return;
  if(!ranking&&phenotypeExploration){
    const diseases=phenotypeExploration.diseases||[],shown=diseases.slice(0,phenotypeResultLimit);
    host.innerHTML=`<div class="phenotype-ranking-heading"><div><span class="phenotype-results-kicker">Report-only exploration</span><h3>Candidate evidence</h3><p>${Number(phenotypeExploration.associatedDiseases||0).toLocaleString()} HPO disease profiles have Mendelian gene associations overlapping ${Number(phenotypeExploration.reportGeneCount||0).toLocaleString()} genes with exact ALT alleles carried by ${escapeHtml(phenotypeExploration.sampleName)}. This uses literal PASS and the representative transcript effect, but does not evaluate phenotype fit, inheritance, allele frequency, pathogenicity, causality, or disease likelihood.</p></div></div><div class="phenotype-candidate-list">${shown.map(disease=>phenotypeCandidateCard(disease,{reportOnly:true})).join('')}</div>${shown.length<diseases.length?`<button type="button" class="phenotype-more fui-button" data-more-phenotype-results>Show 100 more</button>`:''}`;
    return
  }
  if(!ranking){host.innerHTML='<div class="phenotype-ranking-empty"><h3>No candidate comparison yet</h3><p>Add at least one observed finding and prioritize candidates. If no phenotype profile is available, report associations can be explored separately.</p></div>';return}
  let diseases=[...(ranking.diseases||[])];
  diseases.sort((left,right)=>comparePhenotypeCandidates(left,right,phenotypeResultSort==='overlap'));
  const shown=diseases.slice(0,phenotypeResultLimit);
  const online=ranking.onlineEnrichment,onlineGenes=online?.genes||[],onlineByGene=new Map(onlineGenes.map(gene=>[String(gene.symbol||'').toUpperCase(),gene])),localGeneSymbols=new Set(diseases.flatMap(disease=>(disease.genes||[]).map(gene=>String(gene.symbol||'').toUpperCase()))),additionalOnline=onlineGenes.filter(gene=>!localGeneSymbols.has(String(gene.symbol||'').toUpperCase()));
  const onlineNote=online?`<div class="phenotype-source-note"><b>Monarch suggestions integrated</b><span>Matching genes are labeled in candidate cards. Monarch returned ${onlineGenes.length.toLocaleString()} of at most ${Number(online.resultLimit||50).toLocaleString()} suggestions; its score is not combined with local phenotype similarity.</span></div>${additionalOnline.length?`<details class="phenotype-online-supplement fui-accordion"><summary>Additional Monarch gene suggestions (${additionalOnline.length.toLocaleString()})</summary><div>${additionalOnline.map(gene=>`<span><b>${escapeHtml(gene.symbol)}</b><small>#${Number(gene.rank).toLocaleString()} · ${Number(gene.score).toFixed(3)}</small></span>`).join('')}</div></details>`:''}`:ranking.onlineError?`<p class="phenotype-online-error">${escapeHtml(ranking.onlineError)} Local HPO comparison completed normally.</p>`:'';
  const overlapNote=ranking.sampleName?`Report evidence uses exact genotypes for ${escapeHtml(ranking.sampleName)}, literal PASS, Mendelian gene associations, and representative effects. It does not evaluate inheritance, allele frequency, pathogenicity, or causality.`:'No patient sample was selected, so report evidence was not evaluated.';
  host.innerHTML=`<div class="phenotype-ranking-heading"><div><span class="phenotype-results-kicker">Unified candidate view</span><h3>Candidate evidence</h3><p>${Number(ranking.evaluatedDiseases||diseases.length).toLocaleString()} local HPO disease profiles compared by patient-to-disease Lin similarity. Unrecorded disease findings are treated as unknown; explicitly absent findings are shown separately as potential conflicts. This is an experimental evidence order, not a diagnostic probability or validated clinical ranking. ${overlapNote}</p></div><label><span>Order</span><select class="fui-select" data-phenotype-sort><option value="phenotype" ${phenotypeResultSort==='phenotype'?'selected':''}>Phenotype match</option><option value="overlap" ${phenotypeResultSort==='overlap'?'selected':''}>Group by report support</option></select></label></div>${onlineNote}<div class="phenotype-candidate-list">${phenotypeCandidateList(shown,{onlineByGene,hasReportSample:Boolean(ranking.sampleName)})}</div>${shown.length<diseases.length?`<button type="button" class="phenotype-more fui-button" data-more-phenotype-results>Show 100 more</button>`:''}`
}
async function openPhenotypeDialog(){
  if(!currentResultRun)return;
  const dialog=ensurePhenotypeDialog();
  phenotypeDialogRunId=currentResultRun.id;phenotypeProfile=null;phenotypeExploration=null;phenotypeSampleName='';phenotypeMessage='';phenotypeOnlineConsent=false;phenotypeResultLimit=100;phenotypeSearchResults=[];phenotypeSearchActiveIndex=-1;phenotypeSaveRevision++;
  renderPhenotypeDialog();openFluentDialog(dialog);
  try{
    const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/phenotypes`),body=await response.json();
    if(!response.ok)throw new Error(body.error||'Phenotype profile could not be loaded');
    if(phenotypeDialogRunId!==currentResultRun.id)return;
    const sampleNames=body.sampleNames||[],remembered=phenotypeSampleSelections.get(phenotypeDialogRunId),rankedSample=body.ranking?.sampleName;
    phenotypeSampleName=sampleNames.includes(remembered)?remembered:sampleNames.length===1?sampleNames[0]:sampleNames.includes(rankedSample)?rankedSample:'';
    if(phenotypeSampleName)phenotypeSampleSelections.set(phenotypeDialogRunId,phenotypeSampleName);
    if(body.ranking&&(body.ranking.sampleName||'')!==phenotypeSampleName)body.ranking=null;
    phenotypeOnlineConsent=Boolean(body.ranking?.onlineEnrichment||body.ranking?.onlineError);
    phenotypeProfile=body;renderPhenotypeDialog();queueMicrotask(()=>$('#phenotype-dialog [data-phenotype-search]')?.focus())
  }catch(error){phenotypeProfile={observed:[],excluded:[],ranking:null};phenotypeMessage=error.message;renderPhenotypeDialog();queueMicrotask(()=>$('#phenotype-dialog [data-phenotype-search]')?.focus())}
}

  return{open(run,resources){currentResultRun=run;resourceStates=resources;return openPhenotypeDialog()}}
}
