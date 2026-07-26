import { createPhenotypeFeature } from './app/phenotypes.js';
import { createVariantPresentation } from './app/variant-presentation.js';
import { createResultFilters } from './app/result-filters.js';
import { createFavorOnline, favorFieldPresentation } from './app/favor-online.js';
import { installFluentComponentSystem, openFluentDialog, retainFluentModalFocus } from './app/ui-components.js';

installFluentComponentSystem();

const columnGroups=[
  {label:'Variant',columns:[['chromosome','Chr',true],['position','Pos',true],['reference','Ref',true],['alternate','Alt',true],['variantId','Variant ID',false],['quality','QUAL',false],['filter','Filter',false]]},
  {label:'Selected transcript',columns:[['gene','Gene',true],['geneId','Gene ID',false],['transcriptId','Transcript',false],['consequence','Consequence',true],['impact','Impact',true],['canonical','Canonical',false],['maneSelect','MANE Select',false]]}
];
const columns=columnGroups.flatMap(group=>group.columns);
const coreColumnDetails={
  chromosome:['Chromosome','Chromosome or contig containing the variant, normalized to the report assembly.'],
  position:['Position','One-based genomic position of the variant on the chromosome.'],
  reference:['Reference allele','Reference allele recorded in the input VCF after normalization.'],
  alternate:['Alternate allele','Alternate allele represented by this result row.'],
  variantId:['Variant ID','Variant identifier from the VCF ID field, such as a dbSNP rsID when available.'],
  quality:['Quality (QUAL)','VCF QUAL score expressing confidence in the variant call.'],
  filter:['VCF filter','VCF FILTER status, such as PASS or the name of a failed calling filter.'],
  gene:['Gene symbol','Human-readable gene symbol selected for the representative transcript.'],
  geneId:['Ensembl gene ID','Stable Ensembl identifier for the selected gene.'],
  transcriptId:['Transcript ID','Ensembl transcript selected to summarize this variant.'],
  consequence:['Consequence','Most relevant predicted Sequence Ontology consequence for the selected transcript.'],
  impact:['Impact','fastVEP impact category: HIGH, MODERATE, LOW, or MODIFIER.'],
  canonical:['Canonical transcript','Whether the selected transcript is marked canonical by Ensembl.'],
  maneSelect:['MANE Select','Matched Annotation from NCBI and EMBL-EBI transcript when one is available.']
};
const coreFilterColumns=[
  {key:'chromosome',label:'Chromosome',type:'text'},{key:'position',label:'Position',type:'number'},
  {key:'reference',label:'Reference',type:'text'},{key:'alternate',label:'Alternate',type:'text'},
  {key:'variantId',label:'Variant ID',type:'text'},{key:'quality',label:'QUAL',type:'number'},
  {key:'filter',label:'VCF FILTER',type:'text'},{key:'gene',label:'Gene symbol / gene list',type:'text'},
  {key:'geneId',label:'Gene ID',type:'text'},{key:'transcriptId',label:'Transcript',type:'text'},
  {key:'consequence',label:'Consequence',type:'text'},{key:'impact',label:'Impact',type:'text'},
  {key:'canonical',label:'Canonical',type:'boolean'},{key:'maneSelect',label:'MANE Select',type:'text'}
];
const filterOperators=[['equals','='],['not_equals','≠'],['gt','>'],['gte','≥'],['lt','<'],['lte','≤'],['contains','contains'],['not_contains','does not contain'],['in','is in comma-separated list']];
const numericFilterOperators=new Set(['gt','gte','lt','lte']);
const FILTER_PRESET_STORAGE_KEY='annocat.savedResultFilters.v1';
const DISMISSED_COMPLETED_TASKS_STORAGE_KEY='annocat.dismissedCompletedTasks.v1';
const resultQuerySession=typeof globalThis.crypto?.randomUUID==='function'?globalThis.crypto.randomUUID():`${Date.now()}-${Math.random()}`;
const RESULT_PAGE_MEMORY_LIMIT=12,resultPageMemory=new Map(),RESULT_VIEW_MEMORY_LIMIT=4,resultViewMemory=new Map(),VARIANT_DETAIL_MEMORY_LIMIT=64,variantDetailMemory=new Map();
const pageNames={annotate:'New annotation',browse:'Browse results',results:'Results',logs:'Tasks',resources:'Data sources',settings:'Settings'};
let variants=[],sources=[],profiles=[],resourcePlan={resources:[]},evidenceCalibrations={interpretationPolicy:{},predictors:[],calibrations:[],displayPolicies:{}},portablePaths={},visible=new Set(columns.filter(([, ,shown])=>shown).map(([key])=>key)),visibleEvidence=new Set(),resultColumnOrder=[],currentStep=1,selectedPaths=[],selectedVcfSummaries=[],recoveryFiles=null;
let humanReadableColumnNames=localStorage.getItem('annocat.humanReadableColumnNames')!=='false',resultSorts=[];
let setupDismissed=false,lastTaskSnapshots=[],lastAnnotationState={state:'idle'},globalStatusNotice=null,completedRuns=[],lastSetupReady=false,resourceStates={},refreshingResources=false,currentResultRun=null,resultView='all',candidateAlleles=new Set(),resultOffset=0,resultTotal=0,resultCountSignature='',resultNaturalOrderSignature='',resultNaturalOrder=new Map(),selectedAlleleId=null,resultFieldCatalog=[],resultAlignmentGroups=[],resultLoading=false,resultHasMore=false,resultRequestGeneration=0,resultQuerySignature='',resultRequestController=null,loadedCaseNotes='',caseNotesTimer=null,caseNotesRunId=null,selectionRunId=null,selectionAnchorIndex=null,selectedAlleles=new Set(),excludedFilteredAlleles=new Set(),selectedVariantGenes=new Map(),selectedVariantRows=new Map(),selectionMode='explicit',selectionFilterSignature='',dbnsfpConfiguration=null,supplementaryFieldConfigurations=new Map();
let resultSearchTimer=null,resultOperation='',resultQueryError='',favorResultStatus=null,favorResultStatusTimer=null;
let favorOnline={initialize:async()=>{},updateControls:()=>{},updateForRun:async()=>{},isEnabled:()=>true,resetConfirmation:()=>{}};
const RESULT_COLUMN_SELECTION_STORAGE_KEY='annocat.resultColumnSelections.v1';

function resultFieldIdentity(field){return`${field.scope||''}\u001f${field.sourceId||''}\u001f${field.fieldPath||''}`}
function fieldSourceIs(field,id){const source=String(field?.sourceId||'').toLowerCase();return source===id||source.startsWith(`${id}-`)||source.startsWith(`${id}@`)}
function selectableEvidenceField(field,index){
  if(field.scope!=='transcript')return true;
  return!resultFieldCatalog.some((candidate,candidateIndex)=>candidateIndex!==index&&candidate.scope==='allele'&&candidate.sourceId===field.sourceId&&candidate.fieldPath===field.fieldPath)
}
function selectableEvidenceEntries(){return resultFieldCatalog.map((field,index)=>({field,index})).filter(({field,index})=>selectableEvidenceField(field,index))}
function recallResultPage(key){const page=resultPageMemory.get(key);if(!page)return null;resultPageMemory.delete(key);resultPageMemory.set(key,page);return page}
function rememberResultPage(key,page){resultPageMemory.delete(key);resultPageMemory.set(key,page);while(resultPageMemory.size>RESULT_PAGE_MEMORY_LIMIT)resultPageMemory.delete(resultPageMemory.keys().next().value)}
function recallVariantDetail(key){const detail=variantDetailMemory.get(key);if(!detail)return null;variantDetailMemory.delete(key);variantDetailMemory.set(key,detail);return detail}
function rememberVariantDetail(key,detail){variantDetailMemory.delete(key);variantDetailMemory.set(key,detail);while(variantDetailMemory.size>VARIANT_DETAIL_MEMORY_LIMIT)variantDetailMemory.delete(variantDetailMemory.keys().next().value)}
function resultSchemaSelectionKey(){let hash=2166136261;const signature=resultFieldCatalog.map(resultFieldIdentity).sort().join('\u001e');for(let index=0;index<signature.length;index++){hash^=signature.charCodeAt(index);hash=Math.imul(hash,16777619)}return`${resultFieldCatalog.length}-${(hash>>>0).toString(16)}`}
function storedResultColumnSelections(){try{const value=JSON.parse(localStorage.getItem(RESULT_COLUMN_SELECTION_STORAGE_KEY)||'{}');return value&&typeof value==='object'&&!Array.isArray(value)?value:{}}catch{return{}}}
function recommendedEvidenceIndexes(){
  const entries=selectableEvidenceEntries(),selected=[];
  const pick=(...tests)=>{
    for(const test of tests){
      const match=entries.find(({field})=>test(field));
      if(match&&!selected.includes(match.index)){selected.push(match.index);return}
    }
  };
  const leaf=field=>String(field.fieldPath||'').split(/[.\[\]]/).filter(Boolean).pop()?.toLowerCase()||'';
  pick(
    field=>fieldSourceIs(field,'clinvar')&&field.scope==='allele'&&leaf(field)==='significance',
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='clinicalsignificance'
  );
  pick(
    field=>String(field.sourceId||'').toLowerCase().includes('gnomad')&&['allaf','af','allele_frequency'].includes(leaf(field)),
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='gnomadaf'
  );
  pick(
    field=>fieldSourceIs(field,'cadd')&&leaf(field)==='phred',
    field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='cadd_phred',
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='caddphred'
  );
  pick(
    field=>fieldSourceIs(field,'revel')&&leaf(field)==='score',
    field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='revel_score',
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='revel'
  );
  pick(
    field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='alphamissense_score',
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='alphamissense'
  );
  pick(
    field=>fieldSourceIs(field,'phylop')&&leaf(field)==='score',
    field=>fieldSourceIs(field,'dbnsfp')&&leaf(field).includes('phylop'),
    field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='apcconservation'
  );
  pick(field=>fieldSourceIs(field,'favor-online')&&leaf(field)==='spliceaidsmax');
  return selected.slice(0,32)
}
function resultColumnOrderToken(key){if(!key.startsWith('evidence:'))return`core:${key}`;const field=resultFieldCatalog[Number(key.slice(9))];return field?`evidence:${resultFieldIdentity(field)}`:key}
function availableResultColumnOrder(){return[...columns.map(([key])=>`core:${key}`),...selectableEvidenceEntries().map(({field})=>`evidence:${resultFieldIdentity(field)}`)]}
function normalizeResultColumnOrder(order=[]){const available=availableResultColumnOrder(),valid=new Set(available),seen=new Set(),normalized=[];for(const token of [...order,...available])if(valid.has(token)&&!seen.has(token)){seen.add(token);normalized.push(token)}return normalized}
function defaultResultColumnOrder(){const core=columns.filter(([, ,shown])=>shown).map(([key])=>`core:${key}`),evidence=recommendedEvidenceIndexes().map(index=>resultColumnOrderToken(`evidence:${index}`));return normalizeResultColumnOrder([...core,...evidence])}
function applyResultColumnSelection(){const all=storedResultColumnSelections(),saved=all[resultSchemaSelectionKey()],validCore=new Set(columns.map(([key])=>key)),selectable=new Set(selectableEvidenceEntries().map(({index})=>index));if(saved){visible=new Set((saved.core||[]).filter(key=>validCore.has(key)));const wanted=new Set(saved.evidence||[]);visibleEvidence=new Set(resultFieldCatalog.map((field,index)=>selectable.has(index)&&wanted.has(resultFieldIdentity(field))?index:null).filter(index=>index!==null).slice(0,32));resultColumnOrder=normalizeResultColumnOrder(saved.order||[]);return}visible=new Set(columns.filter(([, ,shown])=>shown).map(([key])=>key));visibleEvidence=new Set(recommendedEvidenceIndexes());resultColumnOrder=defaultResultColumnOrder()}
function persistResultColumnSelection(){const all=storedResultColumnSelections(),key=resultSchemaSelectionKey();resultColumnOrder=normalizeResultColumnOrder(resultColumnOrder);all[key]={core:[...visible],evidence:[...visibleEvidence].map(index=>resultFieldCatalog[index]).filter(Boolean).map(resultFieldIdentity),order:resultColumnOrder};const entries=Object.entries(all).slice(-30);localStorage.setItem(RESULT_COLUMN_SELECTION_STORAGE_KEY,JSON.stringify(Object.fromEntries(entries)))}
function restoreDefaultResultColumns(){visible=new Set(columns.filter(([, ,shown])=>shown).map(([key])=>key));visibleEvidence=new Set(recommendedEvidenceIndexes());resultColumnOrder=defaultResultColumnOrder();persistResultColumnSelection();renderColumns();if(currentResultRun){variants=[];renderTable();openCompletedRun(currentResultRun,0)}else renderTable()}
const $=selector=>document.querySelector(selector);
const fileName=path=>path.split(/[\\/]/).pop();
const escapeHtml=value=>String(value).replace(/[&<>"']/g,character=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[character]));
const prototypeIcon=name=>`<svg class="ui-icon prototype-action-icon" aria-hidden="true"><use href="#icon-${name}"></use></svg>`;
const formatDataSize=value=>{if(value===null||value===undefined||value==='')return'';const bytes=Number(value);if(!Number.isFinite(bytes)||bytes<0)return'';const units=['B','KB','MB','GB','TB'];let size=bytes,index=0;while(size>=1000&&index<units.length-1){size/=1000;index++}return`${size.toFixed(index===0?0:size>=100?0:size>=10?1:2)} ${units[index]}`};
const formatResultBytes=formatDataSize;
function showPage(name){document.querySelectorAll('.page').forEach(page=>page.classList.toggle('active-page',page.id===name));document.querySelectorAll('.nav-item').forEach(button=>button.classList.toggle('active',button.dataset.page===name||(name==='results'&&button.dataset.page==='browse')));if(name!=='results')$('#case-notes-panel')?.classList.add('hidden');$('#crumb').textContent=pageNames[name]}
function setSidebarCollapsed(collapsed){document.body.classList.toggle('sidebar-collapsed',collapsed);const button=$('#sidebar-toggle'),label=button.querySelector('span'),icon=button.querySelector('use'),action=collapsed?'Expand navigation':'Collapse navigation';button.setAttribute('aria-expanded',String(!collapsed));button.setAttribute('aria-label',action);button.title=action;if(label)label.textContent=action;if(icon)icon.setAttribute('href',collapsed?'#icon-chevron-right':'#icon-chevron-left');document.querySelectorAll('.nav-item').forEach(item=>{const name=item.querySelector('.nav-label')?.textContent||'',status=item.dataset.statusLabel;item.title=collapsed?`${name}${status?`, ${status}`:''}`:''});$('#about-button').title=collapsed?'About AnnoCAT':'';localStorage.setItem('annocat.sidebarCollapsed',String(collapsed))}
function enableSidebarResize(){const sidebar=$('#sidebar-navigation'),handle=$('#sidebar-resizer');if(!sidebar||!handle)return;const minimum=176,maximum=360,collapseThreshold=112,defaultWidth=document.documentElement.classList.contains('annocat-results-ui')?216:248,applyWidth=(width,persist=false)=>{const value=Math.round(Math.max(minimum,Math.min(maximum,width)));document.body.style.setProperty('--annocat-sidebar-width',`${value}px`);handle.setAttribute('aria-valuenow',String(value));if(persist)localStorage.setItem('annocat.sidebarWidth',String(value))},stored=Number(localStorage.getItem('annocat.sidebarWidth')),rememberedWidth=()=>{const value=Number(localStorage.getItem('annocat.sidebarWidth'));return Number.isFinite(value)&&value>=minimum?value:defaultWidth};handle.setAttribute('aria-valuemin',String(minimum));handle.setAttribute('aria-valuemax',String(maximum));applyWidth(Number.isFinite(stored)&&stored>=minimum?stored:defaultWidth);handle.addEventListener('pointerdown',event=>{event.preventDefault();handle.setPointerCapture?.(event.pointerId);document.body.classList.add('sidebar-resizing');let collapsedByDrag=document.body.classList.contains('sidebar-collapsed'),lastWidth=rememberedWidth();const move=moveEvent=>{const collapse=moveEvent.clientX<=collapseThreshold;if(collapse!==collapsedByDrag){collapsedByDrag=collapse;setSidebarCollapsed(collapse)}if(!collapse){lastWidth=moveEvent.clientX;applyWidth(lastWidth)}},up=()=>{if(!collapsedByDrag)applyWidth(lastWidth,true);document.body.classList.remove('sidebar-resizing');window.removeEventListener('pointermove',move);window.removeEventListener('pointerup',up);window.removeEventListener('pointercancel',up)};window.addEventListener('pointermove',move);window.addEventListener('pointerup',up);window.addEventListener('pointercancel',up)});handle.addEventListener('keydown',event=>{if(!['ArrowLeft','ArrowRight'].includes(event.key))return;event.preventDefault();if(document.body.classList.contains('sidebar-collapsed')){if(event.key==='ArrowRight'){applyWidth(rememberedWidth());setSidebarCollapsed(false)}return}const current=sidebar.getBoundingClientRect().width;if(event.key==='ArrowLeft'&&current<=minimum){setSidebarCollapsed(true);return}applyWidth(current+(event.key==='ArrowLeft'?-12:12),true)})}
setSidebarCollapsed(localStorage.getItem('annocat.sidebarCollapsed')==='true');
enableSidebarResize();
$('#sidebar-toggle').addEventListener('click',()=>setSidebarCollapsed(!document.body.classList.contains('sidebar-collapsed')));
function enableDetailResize(){
  if(!document.documentElement.classList.contains('annocat-results-ui'))return;
  const storageKey='annocat.detailWidth.v3',layout=$('#results .results-layout'),detail=$('#variant-detail'),tableWrap=$('#results .table-wrap'),toolbar=$('#results .toolbar'),tabs=$('#results .result-view-tabs');if(!layout||!detail)return;
  sessionStorage.removeItem('annocat.detailWidth');
  sessionStorage.removeItem('annocat.detailWidth.v2');
  if(tableWrap&&toolbar&&tabs&&!detail.dataset.headingAlignment){const alignHeading=()=>{const tableBounds=tableWrap.getBoundingClientRect(),borderTop=parseFloat(getComputedStyle(tableWrap).borderTopWidth)||0,height=Math.max(45,tableBounds.top+borderTop-layout.getBoundingClientRect().top);detail.style.setProperty('--annocat-detail-heading-height',`${height}px`)};const observer=new ResizeObserver(()=>requestAnimationFrame(alignHeading));[layout,tableWrap,toolbar,tabs].forEach(element=>observer.observe(element));detail.dataset.headingAlignment='true';alignHeading()}
  if(detail.querySelector('.variant-detail-resizer'))return;
  const handle=document.createElement('span');handle.className='variant-detail-resizer';handle.setAttribute('role','separator');handle.setAttribute('aria-label','Resize variant details');handle.setAttribute('aria-orientation','vertical');handle.tabIndex=0;handle.title='Drag to resize variant details; double-click to reset';detail.prepend(handle);
  const applyWidth=width=>{const bounds=layout.getBoundingClientRect(),maximum=Math.max(420,Math.min(720,bounds.width*.62,bounds.width-300)),value=Math.round(Math.max(320,Math.min(maximum,width)));layout.style.setProperty('--annocat-detail-width',`${value}px`);handle.setAttribute('aria-valuenow',String(value));sessionStorage.setItem(storageKey,String(value))};
  const stored=Number(sessionStorage.getItem(storageKey));if(Number.isFinite(stored)&&stored>0)applyWidth(stored);
  handle.addEventListener('pointerdown',event=>{event.preventDefault();const move=moveEvent=>applyWidth(layout.getBoundingClientRect().right-moveEvent.clientX),up=()=>{window.removeEventListener('pointermove',move);window.removeEventListener('pointerup',up)};window.addEventListener('pointermove',move);window.addEventListener('pointerup',up)});
  handle.addEventListener('keydown',event=>{if(!['ArrowLeft','ArrowRight'].includes(event.key))return;event.preventDefault();const current=detail.getBoundingClientRect().width;applyWidth(current+(event.key==='ArrowLeft'?20:-20))});
  handle.addEventListener('dblclick',()=>{layout.style.removeProperty('--annocat-detail-width');sessionStorage.removeItem(storageKey)});
}
enableDetailResize();
let aboutReturnFocus=null;
async function loadAboutMetadata(){
  try{
    const response=await fetch('/api/about'),body=await response.json();
    if(!response.ok)throw new Error(body.error||'Version information is unavailable');
    $('#about-version').textContent=body.version||'Unknown';
  }catch{$('#about-version').textContent='Unknown'}
}
function openAbout(){aboutReturnFocus=document.activeElement;const overlay=$('#about-overlay');overlay.classList.remove('hidden');overlay.classList.add('visible');document.body.classList.add('modal-open');requestAnimationFrame(()=>$('#about-dialog').focus({preventScroll:true}));loadAboutMetadata()}
function closeAbout(){const overlay=$('#about-overlay');overlay.classList.add('hidden');overlay.classList.remove('visible');document.body.classList.remove('modal-open');if(aboutReturnFocus?.focus)aboutReturnFocus.focus()}
$('#about-button').addEventListener('click',openAbout);
$('#about-close').addEventListener('click',closeAbout);
$('#about-overlay').addEventListener('click',event=>{if(event.target===event.currentTarget)closeAbout()});
document.addEventListener('keydown',event=>{const aboutOpen=!$('#about-overlay').classList.contains('hidden'),setupOpen=!$('#first-run').classList.contains('hidden');if(event.key==='Escape'&&aboutOpen){closeAbout();return}if(aboutOpen)retainFluentModalFocus($('#about-dialog'),event);else if(setupOpen)retainFluentModalFocus($('#first-run .setup-modal'),event)});
function updateSetupModal(){const modal=$('#first-run'),wasHidden=modal.classList.contains('hidden'),hasInstalledResources=Object.values(resourceStates).some(state=>state?.ready),hidden=lastSetupReady||hasInstalledResources||setupDismissed;modal.classList.toggle('hidden',hidden);modal.classList.toggle('visible',!hidden);if(wasHidden&&!hidden)requestAnimationFrame(()=>$('#first-run .setup-modal').focus({preventScroll:true}))}
function resourceTitle(id){return id==='grch38-reference'?'GRCh38 reference':id==='ensembl-gff3'?'Ensembl transcript cache':String(id).startsWith('favor')?'FAVOR':sources.find(source=>source.id===id)?.name||id}
function taskActivityLabel(task){if(task.state!=='running')return{queued:'Queued',validating:'Verifying',cancelling:'Stopping and discarding',interrupted:'Interrupted · Resume available',failed:'Needs attention',paused:'Paused',cancelled:'Paused',downloaded:'Ready to install',ready:'Installed',completed:'Completed'}[task.state]||task.phase||task.state;return{'recovery-scan':'Scanning interrupted output','recovery-input':'Preparing remaining input','recovery-merge':'Joining recovered output','indexing-variants':'Building variant table','indexing-evidence':'Building evidence tables','reconnecting':'Reconnecting','retrying':'Reconnecting','replaying':'Replaying','building-cache':'Building cache','downloading-source-part':'Downloading','downloading':'Downloading','streaming-to-fastvep':'Streaming','validating':'Verifying','reading-index':'Reading index','reading-indexes':'Reading index','publishing':'Publishing'}[task.phase]||(task.kind==='installation'?'Installing':task.kind==='download'?'Downloading':'Annotating')}
function taskJobView(task){
  const active=['queued','running','validating','cancelling','downloaded'].includes(task.state);
  const completed=['ready','completed'].includes(task.state);
  const kind=completed?'completed':active?'active':'failed';
  return{state:taskActivityLabel(task),kind,detail:task.error||task.detail||'',name:task.title||'Task',percent:Number.isFinite(Number(task.percent))?Number(task.percent):null}
}
function taskActionButtons(task,actions=task.availableActions||[]){
  const labels={pause:'Pause',resume:'Resume',install:'Install',cancel:'Cancel',discard:'Cancel'};
  const icons={pause:'pause',resume:'play',install:'download',cancel:'close',discard:'trash-2'};
  return actions
    .filter(action=>action!=='remove')
    .map(action=>{
      const appearance=['resume','install'].includes(action)?'fui-button--primary':['cancel','discard'].includes(action)?'fui-button--danger-subtle':'';
      const label=labels[action]||action;
      return`<button type="button" class="fui-button ${appearance} ${action}" data-job-action="${escapeHtml(action)}">${icons[action]?prototypeIcon(icons[action]):''}<span>${escapeHtml(label)}</span></button>`
    })
    .join('')
}
function confirmDestructiveAction({title,message,confirmLabel,cancelLabel='Keep data'}){
  let dialog=$('#confirm-action-dialog');
  if(!dialog){
    document.body.insertAdjacentHTML('beforeend','<dialog id="confirm-action-dialog" class="confirmation-dialog fui-dialog" tabindex="-1" aria-labelledby="confirm-action-title"><form method="dialog"><header class="fui-dialog__header"><div><p class="kicker">Confirm action</p><h2 id="confirm-action-title"></h2></div></header><div class="fui-dialog__content"><p class="fui-dialog__description" data-confirm-action-message></p></div><footer class="fui-dialog__footer"><button type="submit" value="cancel" class="fui-button" data-confirm-action-cancel>Cancel</button><button type="submit" value="confirm" class="fui-button fui-button--danger" data-confirm-action-confirm>Confirm</button></footer></form></dialog>');
    dialog=$('#confirm-action-dialog')
  }
  dialog.querySelector('#confirm-action-title').textContent=title;
  dialog.querySelector('[data-confirm-action-message]').textContent=message;
  dialog.querySelector('[data-confirm-action-cancel]').textContent=cancelLabel;
  dialog.querySelector('[data-confirm-action-confirm]').textContent=confirmLabel;
  dialog.returnValue='';
  return new Promise(resolve=>{
    dialog.addEventListener('close',()=>resolve(dialog.returnValue==='confirm'),{once:true});
    openFluentDialog(dialog)
  })
}
function formatEta(seconds){const value=Math.max(0,Number(seconds)||0);if(value<60)return`about ${Math.ceil(value)} seconds remaining`;if(value<3600)return`about ${Math.ceil(value/60)} minutes remaining`;return`about ${(value/3600).toFixed(value>=36000?0:1)} hours remaining`}
function annotationTaskMeta(task){if(String(task.phase||'').startsWith('indexing-')||task.phase==='publishing'){const parts=[];if(task.detail)parts.push(task.detail);const completed=Number(task.completedRecords||0),total=Number(task.totalRecords||0),bytes=Number(task.completedBytes||0),recordSpeed=Number(task.throughputRecordsPerSecond||0),byteSpeed=Number(task.throughputBytesPerSecond||0);if(total>0)parts.push(`${completed.toLocaleString()} of ${total.toLocaleString()} variants`);if(bytes>0)parts.push(`${formatDataSize(bytes)} written`);if(recordSpeed>0)parts.push(`${Math.round(recordSpeed).toLocaleString()} variants/s`);else if(byteSpeed>0)parts.push(`${formatDataSize(byteSpeed)}/s`);if(task.etaSeconds!==null&&task.etaSeconds!==undefined&&Number(task.etaSeconds)>0)parts.push(formatEta(task.etaSeconds));return parts.join(' · ')}const parts=[];if(task.chromosome)parts.push(`Chromosome ${task.chromosome}`);const completed=Number(task.completedRecords||0),total=Number(task.totalRecords||0);if(total>0)parts.push(`${completed.toLocaleString()} of ${total.toLocaleString()} variants`);const recordSpeed=Number(task.throughputRecordsPerSecond||0),byteSpeed=Number(task.throughputBytesPerSecond||0);if(recordSpeed>0)parts.push(`${Math.round(recordSpeed).toLocaleString()} variants/s`);if(byteSpeed>0)parts.push(`${formatDataSize(byteSpeed)}/s`);if(task.etaSeconds!==null&&task.etaSeconds!==undefined&&Number(task.etaSeconds)>0)parts.push(formatEta(task.etaSeconds));return parts.join(' · ')}
function providerErrorSource(message){return String(message||'').match(/^([a-z0-9-]+) is selected but /i)?.[1]||null}
function renderAnnotationNotice(){const notice=$('#annotation-notice');if(!notice)return;if(globalStatusNotice?.kind!=='annotation'){notice.classList.add('hidden');notice.innerHTML='';return}const sourceId=providerErrorSource(globalStatusNotice.message);notice.innerHTML=`<strong>Annotation could not start</strong><p>${escapeHtml(globalStatusNotice.message)}</p><div class="global-status-actions">${sourceId?`<button type="button" class="fui-button fui-button--small" data-status-disable-source="${escapeHtml(sourceId)}">Continue without ${escapeHtml(resourceTitle(sourceId))}</button>`:''}<button type="button" class="fui-button fui-button--small" data-status-page="resources">Manage data sources</button><button type="button" class="fui-button fui-button--small" data-status-dismiss>Dismiss</button></div>`;notice.classList.remove('hidden')}
function renderGlobalStatus(){
  const button=$('#tasks-nav-button'),indicator=$('#task-nav-status'),count=$('#task-nav-count');
  const views=lastTaskSnapshots.map(task=>taskJobView(task)),active=views.filter(view=>view.kind==='active').length,attention=views.filter(view=>view.kind==='failed').length,total=active+attention;
  if(button&&indicator&&count){
    const parts=[];
    if(active)parts.push(`${active} active task${active===1?'':'s'}`);
    if(attention)parts.push(`${attention} task${attention===1?' needs':'s need'} attention`);
    const status=parts.join(', ');
    indicator.classList.toggle('hidden',total===0);
    indicator.classList.toggle('active',total>0&&attention===0);
    indicator.classList.toggle('attention',attention>0);
    indicator.title=status;
    count.textContent=total>99?'99+':String(total);
    button.dataset.statusLabel=status;
    button.setAttribute('aria-label',status?`Tasks, ${status}`:'Tasks');
    button.title=document.body.classList.contains('sidebar-collapsed')?`Tasks${status?`, ${status}`:''}`:''
  }
  renderAnnotationNotice();
}
function setAnnotationStartError(message){globalStatusNotice={kind:'annotation',title:'Annotation could not start',message:String(message)};renderGlobalStatus();showPage('annotate');setStep(4)}
function clearGlobalStatusNotice(){globalStatusNotice=null;renderAnnotationNotice();renderGlobalStatus()}
function formatDateTime(value){const date=value instanceof Date?value:new Date(value);return Number.isNaN(date.getTime())?'Unknown date':new Intl.DateTimeFormat(undefined,{dateStyle:'medium',timeStyle:'short'}).format(date)}
function taskStateTextClass(kind){return kind==='active'?'fui-text--accent':kind==='completed'?'fui-text--success':kind==='failed'?'fui-text--danger':''}
function dismissedCompletedTaskIds(){
  try{
    const ids=JSON.parse(localStorage.getItem(DISMISSED_COMPLETED_TASKS_STORAGE_KEY)||'[]');
    return new Set(Array.isArray(ids)?ids:[])
  }catch{
    return new Set()
  }
}
function dismissCompletedTasks(){
  const dismissed=dismissedCompletedTaskIds();
  lastTaskSnapshots.filter(task=>taskJobView(task).kind==='completed').forEach(task=>dismissed.add(task.id));
  localStorage.setItem(DISMISSED_COMPLETED_TASKS_STORAGE_KEY,JSON.stringify([...dismissed].slice(-500)));
  renderJobs()
}
function taskCardHtml(task,view){
  if(task.resourceId&&view.kind!=='completed')return resourceTaskHtml(task);
  const annotation=task.kind==='annotation';
  const meta=annotation?annotationTaskMeta(task):'';
  const percent=Math.max(0,Math.min(100,Number(task.percent)||0));
  const actions=annotation?taskActionButtons(task):task.resourceId?taskActionButtons(task):'';
  const timestamp=task.updatedAt?formatDateTime(task.updatedAt):view.kind==='completed'?'Installed':'';
  return`<article class="download-job log-job-card task-state-${view.kind} fui-card" ${task.resourceId?`data-download-job="${escapeHtml(task.resourceId)}"`:''}${annotation?` data-annotation-task="${escapeHtml(task.runId||'')}"`:''}><div class="download-job-head"><div><strong class="fui-card__label">${escapeHtml(view.name)}</strong><small class="fui-caption ${taskStateTextClass(view.kind)}">${escapeHtml(view.state)}</small></div><div class="download-job-actions">${timestamp?`<time class="fui-caption">${escapeHtml(timestamp)}</time>`:''}${actions}</div></div>${annotation&&percent>0&&view.kind!=='completed'?`<div class="download-progress-meta fui-caption"><span>${escapeHtml(meta||view.detail)}</span><strong>${percent.toFixed(1)}%</strong></div><div class="progress-track"><div class="progress-fill" style="width:${percent}%"></div></div>`:`<div class="download-detail fui-caption"><span>${escapeHtml(meta||view.detail)}</span></div>`}</article>`
}
function taskSectionHtml(kind,title,jobs){
  if(!jobs.length)return'';
  const clear=kind==='completed'?'<button type="button" class="fui-button fui-button--subtle fui-button--small" data-clear-completed-tasks>Clear completed</button>':'';
  const description=kind==='failed'?'<p class="fui-caption">Resume recoverable work or cancel to discard its partial data.</p>':'';
  return`<section class="fui-list-section task-section task-section-${kind}" aria-labelledby="task-section-${kind}-title"><header class="fui-list-section__header"><div><span class="fui-list-section__title-row"><h2 id="task-section-${kind}-title">${title}</h2><span class="fui-badge">${jobs.length}</span></span>${description}</div>${clear}</header><div class="fui-list-section__content">${jobs.map(({task,view})=>taskCardHtml(task,view)).join('')}</div></section>`
}
function renderJobs(){
  const dismissed=dismissedCompletedTaskIds();
  const jobs=lastTaskSnapshots.map(task=>({task,view:taskJobView(task)}));
  const activeJobs=jobs.filter(({view})=>view.kind==='active');
  const attentionJobs=jobs.filter(({view})=>view.kind==='failed');
  const completedJobs=jobs.filter(({task,view})=>view.kind==='completed'&&!dismissed.has(task.id));
  const sections=[
    taskSectionHtml('active','Active',activeJobs),
    taskSectionHtml('failed','Needs attention',attentionJobs),
    taskSectionHtml('completed','Completed',completedJobs)
  ].filter(Boolean);
  $('#jobs-list').innerHTML=sections.length?sections.join(''):'<div class="empty-card compact fui-card"><span>✓</span><h2>No tasks to review</h2><p>Active work, items needing attention, and new completions will appear here.</p></div>';
  renderGlobalStatus()
}
$('#jobs-list').addEventListener('click',event=>{
  if(event.target.closest('[data-clear-completed-tasks]'))dismissCompletedTasks()
});
function renderCompletedRuns(runs){const host=$('#completed-runs');host.innerHTML=runs.length?runs.map(run=>{const size=formatResultBytes(run.canonicalResultBytes);return`<button type="button" class="completed-run fui-card fui-card--interactive fui-card--row" data-completed-run="${escapeHtml(run.id)}"><span><strong>${escapeHtml(run.name)}</strong><small>${escapeHtml(formatDateTime(run.completedAt))} · ${escapeHtml(run.assembly)} · ${Number(run.variantCount).toLocaleString()} variants${size?` · ${size} canonical results`:''}</small></span><b>Open →</b></button>`}).join(''):'<div class="empty-card compact fui-card"><span>□</span><h2>No completed annotations yet</h2><p>Finished annotations will appear here automatically.</p></div>';host.querySelectorAll('[data-completed-run]').forEach(button=>button.addEventListener('click',()=>openCompletedRun(runs.find(item=>item.id===button.dataset.completedRun))))}
function renderResultViewTabs(){const candidates=resultView==='candidates';$('#all-variants-tab').classList.toggle('active',!candidates);$('#all-variants-tab').setAttribute('aria-selected',String(!candidates));$('#candidates-tab').classList.toggle('active',candidates);$('#candidates-tab').setAttribute('aria-selected',String(candidates));$('#candidate-count').textContent=candidateAlleles.size.toLocaleString()}
async function loadCandidates(runId){const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/candidates`),body=await response.json();if(!response.ok)throw new Error(body.error||'Candidates could not be loaded');candidateAlleles=new Set((body.candidates||[]).map(candidate=>candidate.alleleId));renderResultViewTabs()}
async function setCandidateMembership(alleleIds,add){if(!currentResultRun||!alleleIds.length)return;let body=null;for(let offset=0;offset<alleleIds.length;offset+=1000){const batch=alleleIds.slice(offset,offset+1000),response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/candidates`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({action:add?'add':'remove',alleleIds:batch})});body=await response.json();if(!response.ok)throw new Error(body.error||'Candidates could not be updated')}resultPageMemory.clear();resultViewMemory.clear();resultCountSignature='';resultNaturalOrderSignature='';resultNaturalOrder.clear();candidateAlleles=new Set((body?.candidates||[]).map(candidate=>candidate.alleleId));renderResultViewTabs();if(resultView==='candidates'){variants=[];await openCompletedRun(currentResultRun,0)}else renderTable()}
function resultViewMemoryKey(view=resultView){return JSON.stringify([currentResultRun?.id||'',view,currentResultFilterSignature(),[...visibleEvidence].sort((a,b)=>a-b),resultSorts])}
function rememberResultView(){if(!currentResultRun||resultLoading)return;const key=resultViewMemoryKey(),table=$('#results .table-wrap');resultViewMemory.delete(key);resultViewMemory.set(key,{variants,resultTotal,resultHasMore,resultOffset,resultCountSignature,resultNaturalOrderSignature,resultNaturalOrder,resultQuerySignature,heading:document.querySelector('#results .results-heading p').textContent,scrollTop:table.scrollTop,scrollLeft:table.scrollLeft});while(resultViewMemory.size>RESULT_VIEW_MEMORY_LIMIT)resultViewMemory.delete(resultViewMemory.keys().next().value)}
function restoreResultView(view){const key=resultViewMemoryKey(view),state=resultViewMemory.get(key);if(!state)return false;resultViewMemory.delete(key);resultViewMemory.set(key,state);({variants,resultTotal,resultHasMore,resultOffset,resultCountSignature,resultNaturalOrderSignature,resultNaturalOrder,resultQuerySignature}=state);resultLoading=false;resultOperation='';resultQueryError='';renderResultViewTabs();renderTable();updateResultPageStatus();updateResultScrollState();document.querySelector('#results .results-heading p').textContent=state.heading;requestAnimationFrame(()=>{const table=$('#results .table-wrap');table.scrollTop=state.scrollTop;table.scrollLeft=state.scrollLeft});return true}
function changeResultView(view){if(!currentResultRun||view===resultView)return;const loadedCandidateRows=view==='candidates'&&!hasActiveResultQuery()&&!resultSorts.length?variants.filter(row=>candidateAlleles.has(row.alleleId)):[];rememberResultView();resultView=view;clearVariantSelection(false);resultRequestController?.abort();if(restoreResultView(view))return;if(view==='candidates'&&loadedCandidateRows.length===candidateAlleles.size){variants=loadedCandidateRows;resultTotal=loadedCandidateRows.length;resultHasMore=false;resultOffset=0;resultCountSignature=currentResultCountSignature();resultNaturalOrderSignature=resultCountSignature;resultNaturalOrder=new Map(variants.map((row,index)=>[row.alleleId,index]));resultLoading=false;resultOperation='';resultQueryError='';renderResultViewTabs();renderTable();updateResultPageStatus();updateResultScrollState();document.querySelector('#results .results-heading p').textContent=`${currentResultRun.assembly} · ${resultTotal.toLocaleString()} candidates · canonical Parquet`;return}resultLoading=true;variants=[];resultTotal=0;resultHasMore=false;renderResultViewTabs();$('#result-page-status').textContent='Loading…';renderTable();openCompletedRun(currentResultRun,0)}
async function openCompletedRun(run,offset=0){
  if(!run||resultLoading&&offset>0)return;
  const switchingRun=currentResultRun?.id!==run.id,appending=offset>0;
  if(switchingRun){
    resultLoading=true;
    resultOperation='Preparing report indexes…';
    resultQueryError='';
    setFavorResultStatus('');
    $('#search').value='';
    caseNotesRunId=null;
    $('#case-notes-panel').classList.add('hidden');
    clearResultFilters(false);
    currentResultRun=run;
    resultView='all';
    favorOnline.updateForRun(run).catch(console.error);
    updateResultPageStatus();
    const detailIndexPromise=fetch(`/api/runs/${encodeURIComponent(run.id)}/detail-index`).then(response=>response.ok).catch(()=>false);
    await loadCandidates(run.id);
    visibleEvidence.clear();
    const fieldResponse=await fetch(`/api/runs/${encodeURIComponent(run.id)}/fields`),fieldBody=await fieldResponse.json();
    if(!fieldResponse.ok)throw new Error(fieldBody.error||'Result columns could not be loaded');
    resultFieldCatalog=fieldBody.fields||[];
    resultAlignmentGroups=fieldBody.alignmentGroups||[];
    applyResultColumnSelection();
    renderColumns();
    renderFilterRules();
    resultSorts=[];
    closeVariantDetail();
    document.querySelector('#results .results-heading h1').textContent=run.name;
    document.querySelector('#results .results-heading p').textContent='Loading the canonical result…';
    variants=[];
    resultTotal=0;
    renderTable();
    await detailIndexPromise;
    resultOperation='Loading…';
  }
  showPage('results');
  if(selectionMode==='filtered'&&selectionFilterSignature!==currentResultFilterSignature())clearVariantSelection(false);
  const search=$('#search').value.trim(),filterParameters=resultFilterParameters(),selectedEvidence=[...visibleEvidence].sort((a,b)=>a-b),filtered=Boolean(search||filterParameters.filterRules.length||filterParameters.evidenceFilters.length),countSignature=currentResultCountSignature(run),querySignature=JSON.stringify([run.id,resultView,search,filterParameters,selectedEvidence,resultSorts]);
  if(appending&&querySignature!==resultQuerySignature)return;
  if(!appending){
    resultRequestController?.abort();
    resultQuerySignature=querySignature;
  }
  const generation=appending?resultRequestGeneration:++resultRequestGeneration,controller=new AbortController();
  resultRequestController=controller;
  if(appending)resultOperation='Loading more…';else if(!resultOperation)resultOperation=resultSorts.length?'Sorting…':filterParameters.filterRules.length||filterParameters.evidenceFilters.length?'Filtering…':search?'Searching…':'Loading…';
  resultQueryError='';
  resultLoading=true;
  resultOffset=Math.max(0,offset);
  updateResultPageStatus();
  updateResultScrollState();
  try{
    const parameters=new URLSearchParams({offset:String(resultOffset),limit:'200',search,querySession:resultQuerySession,requestGeneration:String(generation)}),knownTotal=appending||countSignature===resultCountSignature?resultTotal:0;
    if(knownTotal>0)parameters.set('knownTotal',String(knownTotal));
    if(resultSorts.length)parameters.set('sorts',JSON.stringify(resultSorts.map(({key,direction})=>({column:key,direction}))));
    if(selectedEvidence.length)parameters.set('evidenceColumns',selectedEvidence.join(','));
    if(filterParameters.filterRules.length)parameters.set('filterRules',JSON.stringify(filterParameters.filterRules));
    if(filterParameters.evidenceFilters.length)parameters.set('evidenceFilters',JSON.stringify(filterParameters.evidenceFilters));
    const resultEndpoint=resultView==='candidates'?'candidate-variants':'variants',pageMemoryKey=`${querySignature}\u001f${resultOffset}`,remembered=recallResultPage(pageMemoryKey);let body;
    if(remembered)body=remembered;else{const pageResponse=await fetch(`/api/runs/${encodeURIComponent(run.id)}/${resultEndpoint}?${parameters}`,{signal:controller.signal});body=await pageResponse.json();if(!pageResponse.ok)throw new Error(body.error||'Result could not be opened');rememberResultPage(pageMemoryKey,body)}
    if(generation!==resultRequestGeneration||querySignature!==resultQuerySignature)return;
    const incoming=(body.rows||[]).map(row=>({...row,gene:row.geneSymbol||'',clinvar:'',inheritance:'',score:''}));
    if(appending){
      const known=new Set(variants.map(row=>row.alleleId));
      variants.push(...incoming.filter(row=>!known.has(row.alleleId)))
    }else variants=incoming;
    resultTotal=Number(body.total||0);
    resultCountSignature=countSignature;
    if(!resultSorts.length&&variants.length===resultTotal){resultNaturalOrderSignature=countSignature;resultNaturalOrder=new Map(variants.map((row,index)=>[row.alleleId,index]))}
    resultHasMore=variants.length<resultTotal;
    renderTable();
    document.querySelector('#results .results-heading p').textContent=`${run.assembly} · ${resultTotal.toLocaleString()} ${filtered?'matching ':''}${resultView==='candidates'?'candidates':'variants'} · canonical Parquet`;
  }catch(error){
    if(error.name!=='AbortError'&&generation===resultRequestGeneration&&querySignature===resultQuerySignature){resultQueryError='Result query failed';document.querySelector('#results .results-heading p').textContent=`Could not open this result: ${error.message}`}
  }finally{
    if(generation===resultRequestGeneration&&querySignature===resultQuerySignature){resultLoading=false;resultOperation='';if(resultRequestController===controller)resultRequestController=null;updateResultPageStatus();updateResultScrollState()}
  }
}
async function refreshCurrentResultSchema({sourceId='',preferredFields=[]}={}){
  if(!currentResultRun)return;
  const run=currentResultRun;
  const selectedEvidenceIdentities=new Set([...visibleEvidence].map(index=>resultFieldCatalog[index]).filter(Boolean).map(resultFieldIdentity));
  const sourceWasPresent=Boolean(sourceId)&&resultFieldCatalog.some(field=>fieldSourceIs(field,sourceId));
  const preferredFieldPaths=new Set(preferredFields.map(field=>String(field).toLowerCase()));
  resultRequestController?.abort();
  resultPageMemory.clear();
  resultViewMemory.clear();
  variantDetailMemory.clear();
  const response=await fetch(`/api/runs/${encodeURIComponent(run.id)}/fields`),body=await response.json();
  if(!response.ok)throw new Error(body.error||'Result columns could not be refreshed');
  resultFieldCatalog=body.fields||[];
  resultAlignmentGroups=body.alignmentGroups||[];
  visibleEvidence=new Set(resultFieldCatalog.map((field,index)=>{
    if(selectedEvidenceIdentities.has(resultFieldIdentity(field)))return index;
    if(sourceWasPresent||!fieldSourceIs(field,sourceId))return null;
    return preferredFieldPaths.has(String(field.fieldPath||'').toLowerCase())?index:null
  }).filter(index=>index!==null));
  resultColumnOrder=normalizeResultColumnOrder(resultColumnOrder);
  persistResultColumnSelection();
  renderColumns();
  renderFilterRules();
  resultCountSignature='';
  resultNaturalOrderSignature='';
  resultNaturalOrder.clear();
  resultQuerySignature='';
  variants=[];
  resultLoading=false;
  resultOperation='Loading FAVOR fields…';
  closeVariantDetail();
  await openCompletedRun(run,0)
}
function updateResultScrollState(){const sentinel=$('#result-scroll-sentinel');if(!sentinel)return;sentinel.textContent=resultLoading?(variants.length?'Loading more variants…':''):resultHasMore?'Scroll to load more':variants.length?'All available variants loaded':'';sentinel.classList.toggle('hidden',!sentinel.textContent)}
function loadMoreResults(){if(!currentResultRun||resultLoading||!resultHasMore)return;openCompletedRun(currentResultRun,variants.length)}
async function refreshCompletedRuns(){const response=await fetch('/api/runs'),body=await response.json();if(!response.ok)throw new Error(body.error||'Completed annotations unavailable');completedRuns=body.runs||[];renderCompletedRuns(completedRuns);renderJobs()}
async function openExistingResults(){const response=await fetch('/api/pick-results',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok){document.querySelector('#browse .page-heading>p:last-child').textContent=result.error||'Could not open results';showPage('browse');return}if(result.runId){setupDismissed=true;$('#first-run').classList.add('hidden');await refreshCompletedRuns();const run=completedRuns.find(item=>item.id===result.runId);if(run)await openCompletedRun(run)}}
async function shareCurrentReport(){if(!currentResultRun)return;const button=$('#share-report'),description=document.querySelector('#results .results-heading p'),previous=button.textContent;button.disabled=true;button.textContent='Creating…';try{const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/share`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();if(!response.ok)throw new Error(body.error||'Report could not be created');if(body.path)description.textContent=`Shared report created · ${formatResultBytes(body.bytes)} · ${body.path}`;}catch(error){description.textContent=`Could not share this report: ${error.message}`}finally{button.disabled=false;button.textContent=previous}}
async function renameCurrentReport(){if(!currentResultRun)return;const name=prompt('Report name',currentResultRun.name);if(name===null||name.trim()===currentResultRun.name)return;const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/name`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({name:name.trim()})}),body=await response.json();if(!response.ok){document.querySelector('#results .results-heading p').textContent=`Could not rename this report: ${body.error||'Unknown error'}`;return}currentResultRun.name=body.name;document.querySelector('#results .results-heading h1').textContent=body.name;await refreshCompletedRuns()}
async function loadCaseNotes(){if(!currentResultRun)return;const runId=currentResultRun.id;caseNotesRunId=runId;$('#case-notes-status').textContent='Loading…';const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/notes`),body=await response.json();if(caseNotesRunId!==runId)return;if(!response.ok){$('#case-notes-status').textContent=body.error||'Could not load notes';return}loadedCaseNotes=body.notes||'';$('#case-notes-editor').value=loadedCaseNotes;$('#case-notes-status').textContent='Saved locally'}
async function saveCaseNotes(){const runId=caseNotesRunId;if(!runId)return;clearTimeout(caseNotesTimer);const notes=$('#case-notes-editor').value;$('#case-notes-status').textContent='Saving…';const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/notes`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({notes})}),body=await response.json();if(caseNotesRunId!==runId)return;if(!response.ok){$('#case-notes-status').textContent=body.error||'Could not save notes';return}loadedCaseNotes=notes;$('#case-notes-status').textContent='Saved locally'}
async function toggleCaseNotes(){if(!currentResultRun)return;const panel=$('#case-notes-panel'),opening=panel.classList.contains('hidden');panel.classList.toggle('hidden',!opening);if(opening)await loadCaseNotes()}
const phenotypeFeature=createPhenotypeFeature({$,escapeHtml,prototypeIcon,showPage});
async function openPhenotypeDialog(){return phenotypeFeature.open(currentResultRun,resourceStates)}
function selectionCount(){return selectionMode==='filtered'?Math.max(0,resultTotal-excludedFilteredAlleles.size):selectedAlleles.size}
function displayedSearchEvidenceColumns(search=$('#search').value.trim()){return search?[...visibleEvidence].sort((a,b)=>a-b):[]}
function currentResultFilterSignature(){const search=$('#search').value.trim();return JSON.stringify({search,evidenceColumns:displayedSearchEvidenceColumns(search),...resultFilterParameters()})}
function currentResultCountSignature(run=currentResultRun){return JSON.stringify([run?.id||'',resultView,currentResultFilterSignature()])}
function hasActiveResultQuery(){const filters=resultFilterParameters();return Boolean($('#search').value.trim()||filters.filterRules.length||filters.evidenceFilters.length)}
function setFavorResultStatus(message,{busy=false,tone=''}={}){
  clearTimeout(favorResultStatusTimer);
  favorResultStatus=message?{message,busy,tone}:null;
  updateResultPageStatus();
  if(message&&!busy)favorResultStatusTimer=setTimeout(()=>{favorResultStatus=null;updateResultPageStatus()},tone==='error'?8000:5000)
}
function updateResultPageStatus(){const status=$('#result-page-status');if(!status)return;if(favorResultStatus){status.classList.toggle('error',favorResultStatus.tone==='error');status.innerHTML=`${favorResultStatus.busy?'<i class="result-query-spinner" aria-hidden="true"></i>':''}${escapeHtml(favorResultStatus.message)}`;return}if(resultQueryError){status.textContent=resultQueryError;status.classList.add('error');return}status.classList.remove('error');if(resultLoading){const loaded=variants.length?`${variants.length.toLocaleString()} loaded · `:'';status.innerHTML=`${escapeHtml(loaded)}<i class="result-query-spinner" aria-hidden="true"></i>${escapeHtml(resultOperation||'Loading…')}`;return}status.textContent=resultTotal===0&&hasActiveResultQuery()?'No matching variants':`${variants.length.toLocaleString()} of ${resultTotal.toLocaleString()}`}
function scheduleResultSearch(){clearTimeout(resultSearchTimer);resultRequestController?.abort();resultRequestGeneration++;resultPageMemory.clear();resultQueryError='';resultOperation='Searching…';resultLoading=true;updateResultPageStatus();updateResultScrollState();resultSearchTimer=setTimeout(()=>{if(currentResultRun)openCompletedRun(currentResultRun,0);else{resultLoading=false;resultOperation='';updateResultPageStatus()}},250)}
function updateSelectionControls(){const count=selectionCount(),allFiltered=selectionMode==='filtered',candidateButton=$('#candidate-selected'),candidateLabel=$('#candidate-selected-label'),removeCandidates=resultView==='candidates'||!allFiltered&&count>0&&[...selectedAlleles].every(id=>candidateAlleles.has(id));$('#selection-actions').classList.toggle('hidden',count===0);candidateButton?.classList.toggle('hidden',count===0);if(candidateButton){const action=`${removeCandidates?'Remove':'Add'} ${count.toLocaleString()} selected variant${count===1?'':'s'} ${removeCandidates?'from':'to'} candidates`;candidateLabel.textContent=`${removeCandidates?'Remove from':'Add to'} candidates (${count.toLocaleString()})`;candidateButton.title=action;candidateButton.setAttribute('aria-label',action)}$('#export-selected-genes-label').textContent=count?`Export genes (${count.toLocaleString()})`:'Export genes';$('#export-selected-rows-label').textContent=count?`Export rows (${count.toLocaleString()})`:'Export rows';if(!count){$('#selection-actions-menu').classList.add('hidden');$('#selection-actions-toggle').setAttribute('aria-expanded','false')}favorOnline.updateControls()}
function selectAllFilteredVariants(){if(!resultTotal)return;selectionMode='filtered';selectionFilterSignature=currentResultFilterSignature();selectedAlleles.clear();excludedFilteredAlleles.clear();selectedVariantGenes.clear();selectedVariantRows.clear();renderTable()}
function clearVariantSelection(render=true){selectionMode='explicit';selectionFilterSignature='';selectionAnchorIndex=null;selectedAlleles.clear();excludedFilteredAlleles.clear();selectedVariantGenes.clear();selectedVariantRows.clear();if(render)renderTable();else updateSelectionControls()}
async function filteredAlleleIds(excluded,expectedCount){if(expectedCount>10000)throw new Error('Candidates are limited to 10,000 variants. Narrow the filters before adding this selection.');const ids=[],filters=resultFilterParameters(),search=$('#search').value.trim(),searchEvidence=displayedSearchEvidenceColumns(search),endpoint=resultView==='candidates'?'candidate-variants':'variants';for(let offset=0;offset<resultTotal&&ids.length<expectedCount;){const parameters=new URLSearchParams({offset:String(offset),limit:'500',search});if(searchEvidence.length)parameters.set('evidenceColumns',searchEvidence.join(','));if(filters.filterRules.length)parameters.set('filterRules',JSON.stringify(filters.filterRules));if(filters.evidenceFilters.length)parameters.set('evidenceFilters',JSON.stringify(filters.evidenceFilters));const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/${endpoint}?${parameters}`),body=await response.json();if(!response.ok)throw new Error(body.error||'Selected variants could not be loaded');const rows=body.rows||[];if(!rows.length)break;rows.forEach(row=>{if(row.alleleId&&!excluded.has(row.alleleId))ids.push(row.alleleId)});offset+=rows.length}if(ids.length!==expectedCount)throw new Error(`Expected ${expectedCount.toLocaleString()} variants but loaded ${ids.length.toLocaleString()}. Please try again.`);return ids}
function filteredSelectionAlleleIds(){return filteredAlleleIds(excludedFilteredAlleles,selectionCount())}
async function updateSelectedCandidates(){const count=selectionCount();if(!count)return;const button=$('#candidate-selected'),label=$('#candidate-selected-label'),headerButton=$('#candidate-all'),allFiltered=selectionMode==='filtered',add=allFiltered?resultView!=='candidates':![...selectedAlleles].every(id=>candidateAlleles.has(id));[button,headerButton].filter(Boolean).forEach(item=>item.disabled=true);if(label)label.textContent=add?'Adding…':'Removing…';try{const alleleIds=allFiltered?await filteredSelectionAlleleIds():[...selectedAlleles];await setCandidateMembership(alleleIds,add);clearVariantSelection()}catch(error){document.querySelector('#results .results-heading p').textContent=`Could not update candidates: ${error.message}`}finally{[button,headerButton].filter(Boolean).forEach(item=>item.disabled=false);updateSelectionControls()}}
async function toggleHeaderCandidates(){if(!currentResultRun||!resultTotal)return;const button=$('#candidate-all');button.disabled=true;try{const alleleIds=await filteredAlleleIds(new Set(),resultTotal),add=!alleleIds.every(id=>candidateAlleles.has(id));await setCandidateMembership(alleleIds,add)}catch(error){document.querySelector('#results .results-heading p').textContent=`Could not update candidates: ${error.message}`}finally{const current=$('#candidate-all');if(current)current.disabled=false}}
function setVariantSelected(row,selected){if(!row?.alleleId)return;if(selected){selectedAlleles.add(row.alleleId);selectedVariantGenes.set(row.alleleId,(row.gene||'').trim());selectedVariantRows.set(row.alleleId,{...row})}else{selectedAlleles.delete(row.alleleId);selectedVariantGenes.delete(row.alleleId);selectedVariantRows.delete(row.alleleId)}}
function selectVariantRange(index,{additive=false}={}){if(selectionMode==='filtered')clearVariantSelection(false);if(!additive){selectedAlleles.clear();selectedVariantGenes.clear();selectedVariantRows.clear()}const anchor=selectionAnchorIndex??index,start=Math.min(anchor,index),end=Math.max(anchor,index);for(let item=start;item<=end;item++)setVariantSelected(variants[item],true);selectionAnchorIndex=index;renderTable()}
async function exportFilteredSelection(format){if(!currentResultRun)return;const description=document.querySelector('#results .results-heading p'),columnsToExport=columns.filter(([key])=>visible.has(key)).map(([key])=>key),parameters=resultFilterParameters(),search=$('#search').value.trim(),filters={search,evidenceColumns:displayedSearchEvidenceColumns(search),filterRules:parameters.filterRules,evidenceFilters:parameters.evidenceFilters,excludedAlleleIds:[...excludedFilteredAlleles]};description.textContent=format==='genesTxt'?'Choose where to save all selected genes…':'Choose where to save all selected variants…';try{const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/export`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({format,filters,columns:columnsToExport})}),body=await response.json();if(!response.ok)throw new Error(body.error||'Filtered export failed');if(!body.path){description.textContent='Export cancelled.';return}description.textContent=format==='genesTxt'?`Exported ${Number(body.genes||0).toLocaleString()} unique genes from ${Number(body.rows||0).toLocaleString()} selected variants · ${body.path}`:`Exported ${Number(body.rows||0).toLocaleString()} selected variants · ${body.path}`}catch(error){description.textContent=`Could not export selected results: ${error.message}`}}
function exportFilename(suffix){const name=(currentResultRun?.name||'annocat').replace(/[^a-z0-9_-]+/gi,'-').replace(/^-+|-+$/g,'')||'annocat';return`${name}-${suffix}`}
async function saveExportBlob(blob,filename,description,extension){
  if(window.showSaveFilePicker){
    try{const handle=await window.showSaveFilePicker({suggestedName:filename,types:[{description,accept:{[blob.type.split(';')[0]]:[extension]}}]}),writable=await handle.createWritable();await writable.write(blob);await writable.close();return true}catch(error){if(error.name==='AbortError')return false;throw error}
  }
  const link=document.createElement('a');link.href=URL.createObjectURL(blob);link.download=filename;document.body.appendChild(link);link.click();link.remove();setTimeout(()=>URL.revokeObjectURL(link.href),0);return true
}
async function exportSelectedGenes(){if(selectionMode==='filtered')return exportFilteredSelection('genesTxt');if(!selectedAlleles.size)return;const genes=[],seen=new Set();selectedAlleles.forEach(id=>{const gene=selectedVariantGenes.get(id);if(gene&&!seen.has(gene.toUpperCase())){seen.add(gene.toUpperCase());genes.push(gene)}});if(!genes.length){document.querySelector('#results .results-heading p').textContent='The selected variants do not have gene symbols to export.';return}const description=document.querySelector('#results .results-heading p'),blob=new Blob([`${genes.join(',')}\n`],{type:'text/plain;charset=utf-8'});try{if(!await saveExportBlob(blob,exportFilename('selected-genes.txt'),'Gene list','.txt'))return;description.textContent=`Exported ${genes.length.toLocaleString()} unique gene symbol${genes.length===1?'':'s'} from ${selectedAlleles.size.toLocaleString()} selected variants.`}catch(error){description.textContent=`Could not export selected genes: ${error.message}`}}
function csvCell(value){let text=String(value??'');if(/^[=+\-@]/.test(text))text=`'${text}`;return`"${text.replace(/"/g,'""')}"`}
async function exportSelectedRows(){if(selectionMode==='filtered')return exportFilteredSelection('rowsCsv');if(!selectedAlleles.size)return;const shown=displayColumns(),rows=[shown.map(([,label])=>csvCell(label)).join(',')];selectedAlleles.forEach(id=>{const row=selectedVariantRows.get(id);if(row)rows.push(shown.map(([key])=>csvCell(resultColumnValue(row,key))).join(','))});if(rows.length===1)return;const description=document.querySelector('#results .results-heading p'),blob=new Blob([`\uFEFF${rows.join('\r\n')}\r\n`],{type:'text/csv;charset=utf-8'});try{if(!await saveExportBlob(blob,exportFilename('selected-visible-columns.csv'),'Comma-separated values','.csv'))return;description.textContent=`Exported ${rows.length-1} selected variants with ${shown.length} visible columns.`}catch(error){description.textContent=`Could not export selected rows: ${error.message}`}}
const resultFilters=createResultFilters({
  $,escapeHtml,coreFilterColumns,filterOperators,numericFilterOperators,FILTER_PRESET_STORAGE_KEY,
  selectableEvidenceEntries,coreColumnPresentation,evidenceFieldPresentation,resourceTitle,
  resetResultPages:()=>resultPageMemory.clear(),clearVariantSelection,openCompletedRun,
  getState:()=>({humanReadableColumnNames,resultFieldCatalog,selectionMode,currentResultRun})
});
function resultFilterParameters(...args){return resultFilters.resultFilterParameters(...args)}
function addFilterRule(...args){return resultFilters.addFilterRule(...args)}
function closeFilterColumnPicker(...args){return resultFilters.closeFilterColumnPicker(...args)}
function validateResultFilters(...args){return resultFilters.validateResultFilters(...args)}
function renderFilterRules(...args){return resultFilters.renderFilterRules(...args)}
function filterRulesChanged(...args){return resultFilters.filterRulesChanged(...args)}
function clearResultFilters(...args){return resultFilters.clearResultFilters(...args)}
function refreshFilterPresetSelector(...args){return resultFilters.refreshFilterPresetSelector(...args)}
function saveFilterPreset(...args){return resultFilters.saveFilterPreset(...args)}
function loadFilterPreset(...args){return resultFilters.loadFilterPreset(...args)}
function deleteFilterPreset(...args){return resultFilters.deleteFilterPreset(...args)}
function coreColumnPresentation(key,fallback){const details=coreColumnDetails[key]||[fallback,'Core annotation field in this report.'];return{label:humanReadableColumnNames?details[0]:key,readableLabel:details[0],description:details[1],fieldPath:key,sourceId:'Core annotation'}}
function displayColumns(){const selected=new Map();columns.filter(([key])=>visible.has(key)).forEach(([key,label])=>{const presentation=coreColumnPresentation(key,label);selected.set(`core:${key}`,[key,humanReadableColumnNames?label:key,presentation.description,presentation.fieldPath,presentation.sourceId])});[...visibleEvidence].forEach(index=>{const field=resultFieldCatalog[index];if(!field)return;const presentation=evidenceFieldPresentation(field);selected.set(resultColumnOrderToken(`evidence:${index}`),[`evidence:${index}`,humanReadableColumnNames?presentation.label:field.fieldPath,presentation.description,field.fieldPath,field.sourceId])});const ordered=normalizeResultColumnOrder(resultColumnOrder).filter(token=>selected.has(token));for(const token of selected.keys())if(!ordered.includes(token))ordered.push(token);return ordered.map(token=>selected.get(token))}
function moveResultColumn(sourceKey,targetKey){if(sourceKey===targetKey)return;const shown=displayColumns().map(([key])=>resultColumnOrderToken(key)),source=resultColumnOrderToken(sourceKey),target=resultColumnOrderToken(targetKey),from=shown.indexOf(source),to=shown.indexOf(target);if(from<0||to<0)return;shown.splice(to,0,shown.splice(from,1)[0]);const selected=new Set(shown);resultColumnOrder=[...shown,...normalizeResultColumnOrder(resultColumnOrder).filter(token=>!selected.has(token))];persistResultColumnSelection();renderTable()}
function decodeEvidenceValue(value){if(typeof value!=='string')return value;const text=value.trim();if(!(text.startsWith('[')&&text.endsWith(']')||text.startsWith('{')&&text.endsWith('}')))return value;try{return JSON.parse(text)}catch{return value}}
function resultColumnRawValue(row,key){return key.startsWith('evidence:')?decodeEvidenceValue(row.evidence?.[key.slice(9)]):row[key]}
function resultColumnValue(row,key){if(!key.startsWith('evidence:')){const value=row[key];if(key==='canonical')return value?'Yes':'No';return value??''}const field=resultFieldCatalog[Number(key.slice(9))]||{},value=resultColumnRawValue(row,key);return evidenceValuePresentation({...field,consequenceTerms:row.consequence},value).display}
function resultColumnTooltip(key,description,sourceId){
  if(!key.startsWith('evidence:'))return description;
  const field=resultFieldCatalog[Number(key.slice(9))]||{},presentation=evidenceFieldPresentation(field);
  return[presentation.readingGuide||presentation.baseDescription||description,resourceTitle(sourceId),field.scope==='transcript'?'Selected transcript':'Variant-level',field.fieldPath].filter(Boolean).join(' · ')
}
function renderTableBase(event){
  if(event?.type==='input')return;
  const runId=currentResultRun?.id||null;
  if(selectionRunId!==runId){selectionRunId=runId;clearVariantSelection(false)}
  ['#share-report','#rename-report','#case-notes-button'].forEach(selector=>$(selector).classList.toggle('hidden',!currentResultRun));
  if(!currentResultRun)$('#case-notes-panel').classList.add('hidden');
  const shown=displayColumns(),allFiltered=selectionMode==='filtered',allVisibleCandidates=resultTotal>0&&variants.length>0&&variants.every(row=>candidateAlleles.has(row.alleleId)),candidateAction=allVisibleCandidates?'Remove all filtered variants from candidates':'Add all filtered variants to candidates',emptyMessage=resultLoading?'Loading variants…':hasActiveResultQuery()?'No variants match the current search or filters.':resultView==='candidates'?'No candidates have been added yet.':'This report contains no displayable variants.';
  $('#head').innerHTML=`<th class="selection-cell fui-data-grid__utility-cell"><input id="result-select-all-checkbox" class="fui-checkbox" type="checkbox" aria-label="Select all filtered variants" title="Select or clear all filtered variants"></th><th class="candidate-cell fui-data-grid__utility-cell"><button id="candidate-all" type="button" class="candidate-toggle candidate-column-heading fui-data-grid__icon-button ${allVisibleCandidates?'active':''}" aria-label="${candidateAction}" title="${candidateAction}">${prototypeIcon('star')}<span class="legacy-icon">${allVisibleCandidates?'★':'☆'}</span></button></th>`+shown.map(([key,label,description,,sourceId])=>{const tooltip=resultColumnTooltip(key,description,sourceId),priority=resultSorts.findIndex(sort=>sort.key===key),sort=priority<0?null:resultSorts[priority],indicator=sort?`${resultSorts.length>1?priority+1:''}${sort.direction==='asc'?'▲':'▼'}`:'↕',ariaSort=priority===0?(sort.direction==='asc'?'ascending':'descending'):'none';return`<th title="${escapeHtml(tooltip)}" aria-sort="${ariaSort}"><button type="button" class="column-sort fui-data-grid__sort-button" data-sort-column="${escapeHtml(key)}" aria-label="${escapeHtml(`${label}. ${tooltip} Click to sort; Shift-click to add another sort column.`)}"><span>${escapeHtml(label)}</span><b aria-hidden="true">${indicator}</b></button></th>`}).join('');
  $('#rows').innerHTML=variants.length?variants.map(row=>{const candidate=candidateAlleles.has(row.alleleId),selected=allFiltered?!excludedFilteredAlleles.has(row.alleleId):selectedAlleles.has(row.alleleId),classes=[row.alleleId===selectedAlleleId?'selected-variant':'',selected?'selection-active':''].filter(Boolean).join(' ');return`<tr ${row.alleleId?`tabindex="0" data-allele-id="${escapeHtml(row.alleleId)}" aria-label="Open details for ${escapeHtml(`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`)}"`:''} class="${classes}"><td class="selection-cell fui-data-grid__utility-cell">${row.alleleId?`<input class="fui-checkbox" type="checkbox" data-select-allele="${escapeHtml(row.alleleId)}" aria-label="Select ${escapeHtml(`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`)}" ${selected?'checked':''}>`:''}</td><td class="candidate-cell fui-data-grid__utility-cell">${row.alleleId?`<button type="button" class="candidate-toggle fui-data-grid__icon-button ${candidate?'active':''}" data-toggle-candidate="${escapeHtml(row.alleleId)}" aria-label="${candidate?'Remove from':'Add to'} candidates" title="${candidate?'Remove from':'Add to'} candidates">${prototypeIcon('star')}<span class="legacy-icon">${candidate?'★':'☆'}</span></button>`:''}</td>${shown.map(([key])=>{const value=resultColumnValue(row,key);return key==='impact'?`<td><span class="impact impact-${String(value||'').toLowerCase().replace(/[^a-z0-9_-]/g,'')}">${escapeHtml(value)}</span></td>`:`<td>${escapeHtml(value)}</td>`}).join('')}</tr>`}).join(''):emptyMessage?`<tr class="empty-result-row"><td class="fui-data-grid__empty-cell" colspan="${Math.max(2,shown.length+2)}"><span class="fui-data-grid__empty-content">${escapeHtml(emptyMessage)}</span></td></tr>`:'';
  $('#head').querySelectorAll('[data-sort-column]').forEach(button=>button.addEventListener('click',event=>changeResultSort(button.dataset.sortColumn,event.shiftKey)));
  const selectAll=$('#result-select-all-checkbox');selectAll.checked=allFiltered&&!excludedFilteredAlleles.size;selectAll.indeterminate=allFiltered?excludedFilteredAlleles.size>0:selectedAlleles.size>0;selectAll.disabled=!resultTotal;selectAll.addEventListener('change',()=>selectAll.checked?selectAllFilteredVariants():clearVariantSelection());const candidateAll=$('#candidate-all');candidateAll.disabled=!resultTotal;candidateAll.addEventListener('click',toggleHeaderCandidates);
  updateSelectionControls()
}
function duckDbNumericEvidenceField(field){
  if(['integer','number'].includes(field?.valueType))return true;
  const name=String(field?.fieldPath||'').toLowerCase();
  return name.endsWith('_score')||name.endsWith('_rankscore')||name.endsWith('_phred')||['af','faf','ac','an','dp','gq'].includes(name)||name.includes('allele_frequency')||name.includes('phylop')||name.includes('gerp')
}
function duckDbSortValue(row,key){
  if(key.startsWith('evidence:')){
    const index=Number(key.slice(9)),field=resultFieldCatalog[index],raw=row.evidenceSort?.[index]??row.evidence?.[index],value=Array.isArray(raw)?raw[0]:raw;
    if(value===null||value===undefined)return null;
    if(field?.valueType==='boolean'){if(typeof value==='boolean')return value;const normalized=String(value).toLowerCase();return normalized==='true'?true:normalized==='false'?false:null}
    if(duckDbNumericEvidenceField(field)){const text=(['integer','number'].includes(field?.valueType)?String(value):String(value).split(';',1)[0]).trim();if(!text||text==='.')return null;const number=Number(text);return Number.isFinite(number)?number:null}
    return String(value)
  }
  if(key==='impact')return{HIGH:0,MODERATE:1,LOW:2}[row.impact]??3;
  if(key==='position'||key==='quality')return row[key]===null||row[key]===undefined?null:Number(row[key]);
  if(key==='canonical')return Boolean(row.canonical);
  const value=key==='gene'?row.geneSymbol:row[key];
  return value===null||value===undefined?null:String(value)
}
function compareDuckDbValues(left,right,direction){
  const leftMissing=left===null||left===undefined,rightMissing=right===null||right===undefined;
  if(leftMissing||rightMissing)return leftMissing===rightMissing?0:leftMissing?1:-1;
  const comparison=left===right?0:left<right?-1:1;
  return direction==='desc'?-comparison:comparison
}
function sortFullyLoadedResults(){
  const order=resultNaturalOrder;
  variants.sort((left,right)=>{
    for(const {key,direction} of resultSorts){const comparison=compareDuckDbValues(duckDbSortValue(left,key),duckDbSortValue(right,key),direction);if(comparison)return comparison}
    return(order.get(left.alleleId)??Number.MAX_SAFE_INTEGER)-(order.get(right.alleleId)??Number.MAX_SAFE_INTEGER)
  });
  renderTable()
}
function changeResultSort(key,additive=false){const index=resultSorts.findIndex(sort=>sort.key===key),existing=index<0?null:resultSorts[index];if(additive){if(!existing)resultSorts.push({key,direction:'asc'});else if(existing.direction==='asc')existing.direction='desc';else resultSorts.splice(index,1)}else if(index===0){resultSorts=existing.direction==='asc'?[{key,direction:'desc'}]:[]}else resultSorts=[{key,direction:'asc'}];resultQueryError='';resultOperation='Sorting…';resultLoading=true;updateResultPageStatus();if(!currentResultRun){requestAnimationFrame(()=>{sortFullyLoadedResults();resultLoading=false;resultOperation='';updateResultPageStatus()});return}if(variants.length===resultTotal&&resultTotal<=500&&resultNaturalOrderSignature===currentResultCountSignature()){requestAnimationFrame(()=>{sortFullyLoadedResults();resultLoading=false;resultOperation='';updateResultPageStatus()});return}resultHasMore=false;openCompletedRun(currentResultRun,0)}
let detailCloseTimer,detailOpenFrame;
function revealVariantDetail(){const detail=$('#variant-detail');clearTimeout(detailCloseTimer);cancelAnimationFrame(detailOpenFrame);detail.classList.remove('detail-closing','hidden');if(!detail.classList.contains('detail-visible'))detailOpenFrame=requestAnimationFrame(()=>detail.classList.add('detail-visible'))}
function syncSelectedVariantRow(){document.querySelectorAll('#rows tr[data-allele-id]').forEach(row=>row.classList.toggle('selected-variant',row.dataset.alleleId===selectedAlleleId))}
function closeVariantDetail(){selectedAlleleId=null;syncSelectedVariantRow();const candidateToggle=$('#detail-candidate-toggle'),detail=$('#variant-detail');candidateToggle.classList.add('hidden');delete candidateToggle.dataset.candidateAllele;clearTimeout(detailCloseTimer);cancelAnimationFrame(detailOpenFrame);if(document.documentElement.classList.contains('annocat-results-ui')&&!matchMedia('(prefers-reduced-motion: reduce)').matches&&!detail.classList.contains('hidden')){detail.classList.remove('detail-visible');detail.classList.add('detail-closing');detailCloseTimer=setTimeout(()=>{if(!selectedAlleleId){detail.classList.add('hidden');detail.classList.remove('detail-closing')}},170)}else{detail.classList.remove('detail-visible','detail-closing');detail.classList.add('hidden')}}
function consequenceKeyVariants(name){
  const original=String(name),snake=original.replace(/([a-z0-9])([A-Z])/g,'$1_$2').replace(/[\s-]+/g,'_').toLowerCase(),camel=snake.replace(/_([a-z0-9])/g,(_,character)=>character.toUpperCase());
  return[...new Set([original,snake,camel,snake.toUpperCase()])]
}
function consequenceValue(item,...names){for(const name of names)for(const key of consequenceKeyVariants(name)){const value=item?.[key];if(value!==undefined&&value!==null&&value!=='')return value}return''}
function usefulVariantLinks(row,gene,primary={},variant={}){
  const chromosome=String(row.chromosome||variant.chromosome||'').replace(/^chr/i,'').toUpperCase(),position=Number(row.position||variant.position),reference=String(row.reference||variant.reference||'').toUpperCase(),alternate=String(row.alternate||variant.alternate||'').toUpperCase(),variantId=String(variant.variantId||row.variantId||''),assembly=String(currentResultRun?.assembly||'GRCh38'),geneSymbol=String(gene||'').trim(),rawHgncId=String(consequenceValue(primary,'hgnc_id','HGNC_ID')||'').trim(),hgncMatch=rawHgncId.match(/^(?:HGNC:)?(\d+)$/i),hgncId=hgncMatch?`HGNC:${hgncMatch[1]}`:'',validChromosome=/^(?:[1-9]|1\d|2[0-2]|X|Y)$/.test(chromosome),validPosition=Number.isSafeInteger(position)&&position>0,validAlleles=/^[ACGT]+$/.test(reference)&&/^[ACGT]+$/.test(alternate)&&Math.max(reference.length,alternate.length)<200,isGrch38=/^(?:GRCh38|hg38)$/i.test(assembly),exactSmallVariant=isGrch38&&validChromosome&&validPosition&&validAlleles,locus=`${chromosome}-${position}-${reference}-${alternate}`,links=[];
  if(exactSmallVariant){
    const clinvarQuery=`${locus}(GRCh38)`;
    links.push(
      ['ClinVar',`https://www.ncbi.nlm.nih.gov/clinvar/?term=${encodeURIComponent(clinvarQuery)}`,'Clinical assertions for this exact GRCh38 allele.'],
      ['gnomAD',`https://gnomad.broadinstitute.org/variant/${encodeURIComponent(locus)}?dataset=gnomad_r4`,'Population frequency and quality context for this exact GRCh38 allele.'],
      ['GeneBe',`https://genebe.net/variant/hg38/${encodeURIComponent(`chr${locus}`)}`,'Supplementary annotation and editable automated ACMG assistance for this exact GRCh38 allele.'],
      ['Open Targets',`https://platform.opentargets.org/variant/${encodeURIComponent(`${chromosome}_${position}_${reference}_${alternate}`)}`,'Variant-to-phenotype, functional, and pharmacogenetic evidence when available.']
    )
  }else if(/^rs\d+$/i.test(variantId)){
    links.push(['ClinVar',`https://www.ncbi.nlm.nih.gov/clinvar/?term=${encodeURIComponent(variantId)}`,'ClinVar records associated with this dbSNP identifier.'])
  }
  if(geneSymbol)links.push(['GeneCards',`https://www.genecards.org/card/${encodeURIComponent(geneSymbol)}`,'Gene function, disease, pathway, and identifier summary.']);
  const clingenIdentifier=hgncId||geneSymbol;
  if(clingenIdentifier)links.push(['ClinGen',`https://search.clinicalgenome.org/kb/genes/${encodeURIComponent(clingenIdentifier)}`,'Clinically curated gene-disease validity, dosage sensitivity, and actionability evidence.']);
  return links
}
function displayDetailValue(value){if(value===null||value===undefined||value===''||value==='-')return'—';if(Array.isArray(value))return value.join(', ');if(typeof value==='object')return JSON.stringify(value);return String(value)}
function dbnsfpPredictionValue(field,value){if(value==='.'||value==='')return'Not reported';const labels={VEP_canonical:{YES:'Yes'},GENCODE_basic:{Y:'Yes'}};return labels[field]?.[value]||value}
function renderCandidateDetailControl(alleleId){const button=$('#detail-candidate-toggle');if(!button||!currentResultRun)return;const candidate=candidateAlleles.has(alleleId),label=candidate?'Remove from candidates':'Add to candidates';button.dataset.candidateAllele=alleleId;button.classList.remove('hidden');button.classList.toggle('active',candidate);button.setAttribute('aria-pressed',String(candidate));button.setAttribute('aria-label',label);button.title=label;button.innerHTML=`${prototypeIcon('star')}<span class="legacy-icon">${candidate?'★':'☆'}</span>`}
async function openVariantDetail(alleleId){if(!currentResultRun||!alleleId)return;const runId=currentResultRun.id,row=variants.find(item=>item.alleleId===alleleId);if(!row)return;const detail=$('#variant-detail'),body=$('#variant-detail-body'),opening=detail.classList.contains('hidden')||detail.classList.contains('detail-closing'),title=`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`,cacheKey=`${runId}\u001f${alleleId}`,cached=recallVariantDetail(cacheKey);selectedAlleleId=alleleId;syncSelectedVariantRow();$('#variant-detail-title').textContent=title;if(cached){renderVariantDetail(row,cached);renderCandidateDetailControl(alleleId);return}body.setAttribute('aria-busy','true');if(opening){body.replaceChildren();revealVariantDetail()}try{const locator=new URLSearchParams();if(Number.isSafeInteger(row.recordNumber))locator.set('recordNumber',row.recordNumber);if(Number.isSafeInteger(row.altIndex))locator.set('altIndex',row.altIndex);const suffix=locator.size?`?${locator}`:'';const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/variants/${encodeURIComponent(alleleId)}${suffix}`),result=await response.json();if(!response.ok)throw new Error(result.error||'Variant details unavailable');rememberVariantDetail(cacheKey,result);if(currentResultRun?.id===runId&&selectedAlleleId===alleleId){renderVariantDetail(row,result);renderCandidateDetailControl(alleleId)}}catch(error){if(currentResultRun?.id===runId&&selectedAlleleId===alleleId)body.innerHTML=`<p class="detail-warning">${escapeHtml(error.message)}</p>`}finally{if(currentResultRun?.id===runId&&selectedAlleleId===alleleId)body.removeAttribute('aria-busy')}}
function filterColumnSelector(query){const normalized=query.trim().toLowerCase();$('#column-menu').querySelectorAll('[data-column-group]').forEach(group=>{const groupMatch=group.querySelector('legend')?.textContent.toLowerCase().includes(normalized),fields=[...group.querySelectorAll(':scope>label')];fields.forEach(field=>field.classList.toggle('hidden',Boolean(normalized)&&!groupMatch&&!field.textContent.toLowerCase().includes(normalized)));group.classList.toggle('hidden',fields.every(field=>field.classList.contains('hidden')))})}
function renderColumns(){
  const groups=new Map();
  selectableEvidenceEntries().forEach(({field,index})=>{
    const source=field.sourceId||'other';
    if(!groups.has(source))groups.set(source,[]);
    groups.get(source).push({field,index})
  });
  const search=`<div class="column-menu-searchbar"><input class="column-menu-search fui-input" type="search" data-column-search aria-label="Search displayed columns" placeholder="Search columns, sources, descriptions, or raw keys"></div>`;
  const preference=`<div class="column-menu-toolbar"><label class="column-name-preference"><input class="fui-checkbox" type="checkbox" data-human-readable-columns ${humanReadableColumnNames?'checked':''}><span><strong>Human-readable column names</strong><small>Turn off to show raw report field keys.</small></span></label><button type="button" class="fui-button" data-restore-default-columns>Restore recommended</button></div>`;
  const core=columnGroups.map(group=>`<fieldset data-column-group><legend><label><input class="fui-checkbox" type="checkbox" data-column-group-toggle><span>${escapeHtml(group.label)}</span></label></legend>${group.columns.map(([key,label])=>{const presentation=coreColumnPresentation(key,label);return`<label title="${escapeHtml(presentation.description)}"><input class="fui-checkbox" type="checkbox" data-key="${key}" ${visible.has(key)?'checked':''}><span class="column-field-copy"><strong>${escapeHtml(presentation.label)}</strong><small>${escapeHtml(presentation.description)}</small><code>${escapeHtml(key)}</code></span></label>`}).join('')}</fieldset>`).join('');
  const dynamic=[...groups.entries()].map(([source,fields])=>`<fieldset class="evidence-column-group" data-column-group><legend><label><input class="fui-checkbox" type="checkbox" data-column-group-toggle><span>${escapeHtml(resourceTitle(source))}</span></label></legend>${fields.map(({field,index})=>{const presentation=evidenceFieldPresentation(field),label=humanReadableColumnNames?presentation.label:field.fieldPath;return`<label title="${escapeHtml(presentation.description)}"><input class="fui-checkbox" type="checkbox" data-evidence-index="${index}" ${visibleEvidence.has(index)?'checked':''}><span class="column-field-copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(presentation.description)}</small><code>${escapeHtml(field.fieldPath)} · ${escapeHtml(field.valueType||'unknown')}</code></span></label>`}).join('')}</fieldset>`).join('');
  const menu=$('#column-menu');
  menu.innerHTML=search+`<div class="column-menu-scroll">${preference}${core}${dynamic}</div>`;
  menu.querySelector('[data-column-search]').addEventListener('input',event=>filterColumnSelector(event.target.value));
  menu.querySelector('[data-restore-default-columns]').addEventListener('click',restoreDefaultResultColumns);
  menu.querySelector('[data-human-readable-columns]').addEventListener('change',event=>{
    humanReadableColumnNames=event.target.checked;
    localStorage.setItem('annocat.humanReadableColumnNames',String(humanReadableColumnNames));
    renderColumns();
    renderTable();
    renderFilterRules()
  });
  menu.querySelectorAll('[data-key]').forEach(box=>box.addEventListener('change',event=>{
    event.target.checked?visible.add(event.target.dataset.key):visible.delete(event.target.dataset.key);
    persistResultColumnSelection();
    renderTable()
  }));
  menu.querySelectorAll('[data-evidence-index]').forEach(box=>box.addEventListener('change',event=>{
    const index=Number(event.target.dataset.evidenceIndex);
    if(event.target.checked&&visibleEvidence.size>=32){
      event.target.checked=false;
      document.querySelector('#results .results-heading p').textContent='Up to 32 database columns can be displayed at once.';
      return
    }
    event.target.checked?visibleEvidence.add(index):visibleEvidence.delete(index);
    persistResultColumnSelection();
    variants=[];
    renderTable();
    if(currentResultRun)openCompletedRun(currentResultRun,0)
  }));
  syncColumnGroupToggles()
}
function syncColumnGroupToggles(){
  $('#column-menu').querySelectorAll('[data-column-group]').forEach(group=>{const toggle=group.querySelector('[data-column-group-toggle]'),fields=[...group.querySelectorAll('[data-key],[data-evidence-index]')],checked=fields.filter(field=>field.checked).length;toggle.checked=fields.length>0&&checked===fields.length;toggle.indeterminate=checked>0&&checked<fields.length})
}
function toggleColumnGroup(toggle){
  const fields=[...toggle.closest('[data-column-group]').querySelectorAll('[data-key],[data-evidence-index]')];let evidenceChanged=false,limited=false;
  fields.forEach(field=>{if(field.dataset.key){toggle.checked?visible.add(field.dataset.key):visible.delete(field.dataset.key);field.checked=toggle.checked;return}const index=Number(field.dataset.evidenceIndex);if(toggle.checked&&!visibleEvidence.has(index)&&visibleEvidence.size>=32){limited=true;field.checked=false;return}toggle.checked?visibleEvidence.add(index):visibleEvidence.delete(index);field.checked=toggle.checked;evidenceChanged=true});
  persistResultColumnSelection();syncColumnGroupToggles();renderTable();if(limited)document.querySelector('#results .results-heading p').textContent='Up to 32 database columns can be displayed at once.';if(evidenceChanged){variants=[];if(currentResultRun)openCompletedRun(currentResultRun,0)}
}
$('#column-menu').addEventListener('change',event=>{if(event.target.matches('[data-column-group-toggle]'))toggleColumnGroup(event.target);else if(event.target.matches('[data-key],[data-evidence-index]'))syncColumnGroupToggles()});
function enabledSourceIds(){return[...document.querySelectorAll('#wizard-sources input:checked')].map(input=>input.dataset.source)}
function selectedProfile(){return profiles.find(profile=>profile.id===$('#profile').value)}
function applyProfile(){renderWizardSources();updateWizardReadiness()}
function renderProfiles(){const ordered=[...profiles].sort((a,b)=>a.id==='wgs'?-1:b.id==='wgs'?1:0),preferred=ordered.some(profile=>profile.id==='wgs')?'wgs':ordered[0]?.id;profiles=ordered;$('#profile').innerHTML=ordered.map(profile=>`<option value="${escapeHtml(profile.id)}" ${profile.id===preferred?'selected':''}>${escapeHtml(profile.name)}${profile.id==='wgs'?' (recommended)':''}</option>`).join('')+'<option value="custom">Custom</option>';const host=$('#profile-install-actions'),installProfiles=ordered.filter(profile=>(profile.sourceIds||[]).length||(profile.serviceIds||[]).length===0);host.innerHTML=installProfiles.map(profile=>{const label=profile.name,names=['Core annotation data',...(profile.sourceIds||[]).map(id=>sources.find(source=>source.id===id)?.name||id)];return`<article class="fui-card fui-card--content-compact"><strong class="fui-card__title">${escapeHtml(label)}${profile.id==='wgs'?' · Recommended':''}</strong><small class="fui-card__metadata">${escapeHtml(names.join(' · '))}</small><button type="button" class="fui-button" data-profile-install="${escapeHtml(profile.id)}">Install ${escapeHtml(label)}</button></article>`}).join('');host.querySelectorAll('[data-profile-install]').forEach(button=>button.addEventListener('click',()=>showProfileInstallReview(button.dataset.profileInstall)))}
function sourceLicenseNote(source){if(source.id==='hpo')return'<small class="source-license-note">Uses a versioned Human Phenotype Ontology release. <a href="https://human-phenotype-ontology.github.io/license.html" target="_blank" rel="noopener noreferrer">License and attribution</a>.</small>';if(source.delivery==='managed-public-noncommercial')return'<small class="source-license-note">Non-commercial use; commercial use requires a CADD license.</small>';if(source.delivery==='user-supplied-licensed')return`<small class="source-license-note">Import files obtained under your ${escapeHtml(source.name)} license.</small>`;return''}
function orderedCatalogSources(){const installable=new Set(resourcePlan.resources.filter(resource=>resource.state==='missing').map(resource=>resource.id));return sources.filter(source=>source.id!=='fastvep'&&installable.has(source.id))}
function renderWizardSources(){const recommended=new Set(selectedProfile()?.sourceIds||[]),container=$('#wizard-sources'),availableCatalog=orderedCatalogSources().filter(source=>source.fastvepSource&&resourcePlan.resources.some(item=>item.id===source.id&&item.state==='missing'));container.innerHTML=availableCatalog.map(source=>{const state=resourceStates[source.id],ready=Boolean(state?.ready),isRecommended=recommended.has(source.id),badge=isRecommended?'<small class="profile-badge fui-badge">Profile</small>':'';return`<label class="source-option fui-choice-row ${ready?'':'source-unavailable'}"><input class="fui-checkbox" type="checkbox" data-source="${escapeHtml(source.id)}" ${ready&&isRecommended?'checked':''} ${ready?'':'disabled'}><span><strong>${escapeHtml(source.name)} ${badge}</strong><small>${escapeHtml(source.purpose)}</small></span><em data-resource-state="${escapeHtml(source.id)}">${ready?'Installed':'Not installed'}</em></label>`}).join('');container.querySelectorAll('input[data-source]').forEach(input=>input.addEventListener('change',sourceSelectionChanged))}
function sourceSelectionChanged(event){const input=event.target;if(input.checked&&['gnomad','gnomad-genomes'].includes(input.dataset.source)){const other=$(`#wizard-sources input[data-source="${input.dataset.source==='gnomad'?'gnomad-genomes':'gnomad'}"]`);if(other)other.checked=false}$('#profile').value='custom';$('#wizard-sources .profile-badge').forEach(badge=>badge.remove());updateWizardReadiness()}
function updateWizardReadiness(){const host=$('#wizard-readiness');if(!host)return;const profile=selectedProfile(),selected=enabledSourceIds(),missing=(profile?.sourceIds||[]).filter(id=>!resourceStates[id]?.ready),usesFavor=(profile?.serviceIds||[]).includes('favor-variant-annotation');let tone='ready',title='',detail='';if(!lastSetupReady){tone='blocked';title='Local annotation is not ready';detail='Install the GRCh38 reference and transcript cache before starting an annotation.'}else if(profile&&missing.length){tone='partial';title=`${selected.length} available profile source${selected.length===1?'':'s'} selected`;detail=`${missing.length} profile source${missing.length===1?' is':'s are'} not installed. You can continue with the available sources or manage data sources.`}else if(selected.length){title=`${selected.length} data source${selected.length===1?'':'s'} selected`;detail='The selected sources are installed and ready for annotation.'}else if(usesFavor){tone=favorOnline.isEnabled()?'ready':'partial';title=favorOnline.isEnabled()?'Core annotation with FAVOR enrichment':'Core annotation; FAVOR is disabled';detail=favorOnline.isEnabled()?'FAVOR enrichment will be available for selected or filtered variants in the completed report.':'Enable FAVOR in Data sources to use online enrichment after annotation.'}else{tone='partial';title='Core annotation only';detail='No supplementary data sources are selected.'}const appearance={ready:'success',partial:'info',blocked:'danger'}[tone];host.className=`wizard-readiness fui-status-message fui-status-message--${appearance} ${tone}`;host.querySelector('strong').textContent=title;host.querySelector('p').textContent=detail;host.querySelector('button').classList.toggle('hidden',tone==='ready')}
function selectedVcfProblem(file){if(!file)return null;if(file.error)return`This file could not be read: ${file.error}`;if(file.assembly==='GRCh37')return'GRCh37, b37, and hg19 inputs are not supported in this release. Select a GRCh38 VCF.';if(file.assembly&&file.assembly!=='GRCh38')return`AnnoCAT supports GRCh38 inputs only; this file declares ${file.assembly}.`;return null}
function selectedVcfBlockingProblem(){return selectedVcfSummaries.map((file,index)=>({file,index,problem:selectedVcfProblem(file)})).find(item=>item.problem)||null}
function setStep(step){currentStep=step;document.querySelectorAll('.wizard-panel').forEach(panel=>panel.classList.toggle('active-panel',Number(panel.dataset.step)===step));document.querySelectorAll('.steps li').forEach((item,index)=>{item.classList.toggle('current',index+1===step);item.classList.toggle('complete',index+1<step)});$('#recover-annotation').classList.toggle('hidden',step!==1);$('#back-step').classList.toggle('hidden',step===1);const button=$('#continue'),blocked=selectedVcfBlockingProblem();button.disabled=(step===1&&(!selectedPaths.length||Boolean(blocked)))||(step===3&&!$('#output-folder').value.trim())||step===4;button.innerHTML=step===3?'Review plan <span>→</span>':step===4?'Checking resources…':'Continue <span>→</span>';if(step===2)updateWizardReadiness();if(step===4)renderReview()}
function renderSelectedPaths(){const container=$('#selected-files'),recovery=$('#recovery-selection'),picker=$('#choose-vcfs');picker.querySelector('strong').textContent=recoveryFiles?'Choose original input VCF':'Choose VCF files';picker.querySelector('small').textContent=recoveryFiles?'Select the VCF, VCF.GZ, or BGZ that produced this output':'Select one or more VCF, VCF.GZ, or BGZ files';if(recoveryFiles){const input=recoveryFiles.input?` Original input: ${escapeHtml(fileName(recoveryFiles.input))}.`:' Now choose the original input VCF.';recovery.innerHTML=`<span><svg class="ui-icon"><use href="#icon-info"/></svg></span><p><strong>Recovering ${escapeHtml(fileName(recoveryFiles.partialVcf))}</strong><br>Complete records will be retained.${input} Select the same profile and sources used by the interrupted run.</p>`;recovery.classList.remove('hidden')}else{recovery.classList.add('hidden');recovery.innerHTML=''}if(!selectedPaths.length){container.classList.add('hidden');setStep(1);return}const blocked=selectedVcfBlockingProblem(),problem=blocked?`<div class="batch-problem" role="alert"><strong>Choose a supported input</strong><span>${escapeHtml(blocked.file?.name||fileName(selectedPaths[blocked.index]))}: ${escapeHtml(blocked.problem)}</span></div>`:'';container.innerHTML=`<div class="batch-heading"><strong class="fui-card__label">${recoveryFiles?'Original input':`${selectedPaths.length} VCF${selectedPaths.length===1?'':'s'} selected${selectedPaths.length>1?' · sequential order':''}`}</strong><button id="clear-vcfs" class="fui-button fui-button--transparent fui-button--small" type="button">Clear</button></div>${problem}${selectedPaths.map((path,index)=>{const file=selectedVcfSummaries[index],fileProblem=selectedVcfProblem(file);return`<div class="batch-file${fileProblem?' invalid':''}"><span>${index+1}</span><div><strong class="fui-card__label">${escapeHtml(fileName(path))}</strong><small>${escapeHtml(path)}</small>${fileProblem?`<em>${escapeHtml(fileProblem)}</em>`:''}</div><button class="fui-button fui-button--small fui-button--icon fui-button--subtle" type="button" data-remove="${index}" aria-label="Remove ${escapeHtml(fileName(path))}">×</button></div>`}).join('')}`;container.classList.remove('hidden');$('#clear-vcfs').addEventListener('click',()=>{selectedPaths=[];selectedVcfSummaries=[];recoveryFiles=null;renderSelectedPaths()});container.querySelectorAll('[data-remove]').forEach(button=>button.addEventListener('click',()=>{if(recoveryFiles){selectedPaths=[];selectedVcfSummaries=[];delete recoveryFiles.input}else{const index=Number(button.dataset.remove);selectedPaths.splice(index,1);selectedVcfSummaries.splice(index,1)}renderSelectedPaths()}));setStep(1)}
async function chooseVcfs(){const recovering=Boolean(recoveryFiles),endpoint=recovering?'/api/pick-recovery-input':'/api/pick-vcfs',response=await fetch(endpoint,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not choose VCF files');const paths=recovering?(result.path?[result.path]:[]):result.paths;if(paths?.length){if(recovering){recoveryFiles.input=paths[0];selectedPaths=[paths[0]];selectedVcfSummaries=result.file?[result.file]:[]}else{selectedPaths=paths;selectedVcfSummaries=result.files||[]}renderSelectedPaths()}}
async function chooseRecoveryFiles(){const response=await fetch('/api/pick-recovery-files',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not choose interrupted annotation files');if(result.partialVcf){recoveryFiles=result;selectedPaths=[];selectedVcfSummaries=[];renderSelectedPaths()}}
function resourceSize(id,state=resourceStates[id],formatter=formatDataSize){if(id==='grch38-reference'&&state===resourceStates[id])return coreAnnotationSize(undefined,formatter);const item=resourcePlan.resources.find(resource=>resource.id===id),network=item?.downloadBytes?`${formatter(item.downloadBytes)} network`:item?.state==='catalog-pending'?'Network size pending':'Network size unknown',prepared=Number(state?.prepare?.preparedBytes||0);if(prepared>0)return`${network} · ${formatter(prepared)} cache on disk${state?.ready?'':' so far'}`;return item?.installMode==='stream'?`${network} · cache size measured during install`:network}
function profileInstallSize(item){const prepared=Number(resourceStates[item.id]?.prepare?.preparedBytes||0);if(resourceStates[item.id]?.ready&&prepared>0)return`${formatDataSize(prepared)} on disk`;if(item.downloadBytes)return`${formatDataSize(item.downloadBytes)} download`;return'Size determined during install'}
const coreResourceIds=new Set(['grch38-reference','ensembl-gff3']);
function coreAnnotationSize(items=resourcePlan.resources.filter(item=>coreResourceIds.has(item.id)),formatter=formatDataSize){const networkBytes=items.reduce((sum,item)=>sum+Number(item.downloadBytes||0),0),preparedBytes=items.reduce((sum,item)=>sum+Number(resourceStates[item.id]?.prepare?.preparedBytes||0),0),network=networkBytes?`${formatter(networkBytes)} network`:'Network size unknown';return preparedBytes?`${network} · ${formatter(preparedBytes)} cache on disk`:`${network} · cache size measured during install`}
function renderReview(){const ids=enabledSourceIds(),keepVcf=$('#keep-annotated-vcf').checked;$('#review-summary').innerHTML=`<div class="fui-summary-grid__item"><span>Input</span><strong>${recoveryFiles?'Interrupted annotation recovery':`${selectedPaths.length} VCF${selectedPaths.length===1?'':'s'}`}</strong></div><div class="fui-summary-grid__item"><span>Run order</span><strong>${recoveryFiles?'Resume remaining records':selectedPaths.length===1?'Single run':'Sequential separate runs'}</strong></div><div class="fui-summary-grid__item"><span>Profile</span><strong>${escapeHtml($('#profile').selectedOptions[0].text)}</strong></div><div class="fui-summary-grid__item"><span>Output</span><strong>${escapeHtml($('#output-folder').value||'Not selected')}</strong></div><div class="fui-summary-grid__item"><span>Results</span><strong>Canonical viewer result${keepVcf?' + annotated VCF':''}</strong></div><div class="review-summary-empty fui-summary-grid__item" aria-hidden="true"></div>`;const required=`<div class="fui-key-value-row" data-resource-review="core"><span class="readiness-dot"></span><strong>Core annotation data</strong><small>${escapeHtml(coreAnnotationSize())}</small><em data-reference-state>Checking…</em></div>`;$('#resource-review').innerHTML=required+ids.map(id=>`<div class="fui-key-value-row" data-resource-review="${escapeHtml(id)}"><span class="readiness-dot"></span><strong>${escapeHtml(resourceTitle(id))}</strong><small>${escapeHtml(resourceSize(id))}</small><em data-resource-state="${escapeHtml(id)}">Checking…</em></div>`).join('');updateReviewResourceStates();refreshAppStatus().catch(console.error)}
function updateReviewResourceStates(){const core=$('[data-resource-review="core"]');if(core)core.classList.toggle('ready',lastSetupReady);enabledSourceIds().forEach(id=>document.querySelector(`[data-resource-review="${id}"]`)?.classList.toggle('ready',Boolean(resourceStates[id]?.ready)));updateReviewReadiness()}
function updateReviewReadiness(){const host=$('#review-readiness');if(!host)return;const ids=enabledSourceIds(),selectedReady=ids.every(id=>resourceStates[id]?.ready),ready=lastSetupReady&&selectedReady;host.className=`wizard-readiness fui-status-message fui-status-message--${ready?'success':'danger'} ${ready?'ready':'blocked'}`;host.querySelector('strong').textContent=ready?'Ready to annotate':'Annotation resources are not ready';host.querySelector('p').textContent=ready?`${ids.length?`${ids.length} supplementary source${ids.length===1?'':'s'} plus core annotation data`:'Core annotation data'} will be used.`:'Install the missing core or selected resources before starting.';host.querySelector('button').classList.toggle('hidden',ready)}
async function refreshAnnotationStatus(snapshot){let body=snapshot;if(!body){const response=await fetch('/api/annotations/status');body=await response.json();if(!response.ok)throw new Error(body.error||'Annotation status unavailable')}const previous=lastAnnotationState.state;lastAnnotationState=body;if(previous==='running'&&body.state==='completed'){await refreshCompletedRuns();showPage('browse')}return body}
async function refreshTasks(snapshot){let body=snapshot;if(!body){const response=await fetch('/api/tasks');body=await response.json();if(!response.ok)throw new Error(body.error||'Tasks unavailable')}lastTaskSnapshots=body.tasks||body||[];renderJobs();return lastTaskSnapshots}
async function cancelAnnotation(){await fetch('/api/annotations/cancel',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}});await refreshAnnotationStatus()}
async function handleAnnotationTaskAction(runId,action,button){
  if(action==='cancel'&&!await confirmDestructiveAction({title:'Cancel this annotation?',message:'The annotation will stop and its incomplete output will be discarded. Completed annotations and installed data sources are not affected.',confirmLabel:'Cancel annotation',cancelLabel:'Keep running'}))return;
  if(action==='discard'&&!await confirmDestructiveAction({title:'Cancel this interrupted annotation?',message:'The interrupted run, its checkpoint, and all partial output will be permanently deleted. Completed annotations and installed data sources are not affected.',confirmLabel:'Cancel annotation',cancelLabel:'Keep partial data'}))return;
  const original=button.textContent;
  button.disabled=true;
  button.textContent=action==='resume'?'Resuming…':action==='pause'?'Pausing…':action==='discard'?'Deleting…':'Stopping…';
  try{
    if(action==='resume'){
      const response=await fetch('/api/annotations/resume',{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({runId})}),body=await response.json();
      if(!response.ok)throw new Error(body.error||'Annotation could not resume')
    }else if(action==='pause'){
      const response=await fetch('/api/annotations/pause',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();
      if(!response.ok||!body.pauseRequested)throw new Error(body.error||'Annotation is no longer running')
    }else if(action==='discard'){
      const response=await fetch('/api/annotations/discard',{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({runId})}),body=await response.json();
      if(!response.ok)throw new Error(body.error||'Interrupted annotation could not be deleted')
    }else if(action==='cancel'){
      await cancelAnnotation()
    }
    await refreshAppStatus()
  }catch(error){
    showResourceNotice(error.message)
  }finally{
    button.disabled=false;
    button.textContent=original
  }
}
async function startAnnotation(){const button=$('#continue'),recovering=Boolean(recoveryFiles),sourceIds=enabledSourceIds(),includeAnnotatedVcf=$('#keep-annotated-vcf').checked,blocked=selectedVcfBlockingProblem(),unknownBuild=selectedVcfSummaries.length!==selectedPaths.length||selectedVcfSummaries.some(file=>!file.assembly);if(blocked){setStep(1);return}let confirmGrch38=false;if(unknownBuild){confirmGrch38=await confirmDestructiveAction({title:'Confirm genome build',message:'The VCF header does not identify its genome build. Continue only if this file uses GRCh38 coordinates and reference alleles. AnnoCAT will validate sequence alleles against its installed GRCh38 reference.',confirmLabel:'This is GRCh38',cancelLabel:'Go back'});if(!confirmGrch38)return}const endpoint=recovering?'/api/annotations/recover':'/api/annotations/start',payload=recovering?{input:recoveryFiles.input,partialVcf:recoveryFiles.partialVcf,structuredOutput:recoveryFiles.structuredOutput,outputDirectory:$('#output-folder').value.trim(),sourceIds,includeAnnotatedVcf,confirmGrch38}:{inputs:selectedPaths,outputDirectory:$('#output-folder').value.trim(),sourceIds,includeAnnotatedVcf,confirmGrch38};button.disabled=true;button.textContent=recovering?'Starting verification…':'Starting…';try{const response=await fetch(endpoint,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify(payload)}),body=await response.json();if(!response.ok)throw new Error(body.error||(recovering?'Recovery could not start':'Annotation could not start'));clearGlobalStatusNotice();await refreshAnnotationStatus();showPage('logs')}catch(error){setAnnotationStartError(error.message)}finally{await refreshAppStatus()}}
async function chooseFolder(){const button=$('#browse-output'),message=$('#folder-message');button.disabled=true;button.textContent='Opening…';message.classList.remove('error');try{const response=await fetch('/api/pick-folder',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Native folder picker failed');if(result.path){$('#output-folder').value=result.path;message.textContent=`Selected ${result.path}`;setStep(3)}}catch(error){message.textContent=`Could not open the folder picker: ${error.message}. Start AnnoCAT with “annocat launch” from your PowerShell window.`;message.classList.add('error')}finally{button.disabled=false;button.textContent='Browse…'}}
async function refreshPaths(){
  portablePaths=await fetch('/api/paths').then(response=>response.json());
  document.querySelectorAll('#wizard-resource-path').forEach(element=>element.textContent=portablePaths.resourceDirectory||'Unavailable');
  $('#settings-resource-path').value=portablePaths.resourceDirectory||'Unavailable';
  $('#settings-downloads-path').value=portablePaths.downloads||'Unavailable';
  $('#settings-results-path').value=portablePaths.runs||'Unavailable';
  return portablePaths
}
const settingsStoragePickers={
  resource:{route:'resource',message:'Contains installed annotation caches.'},
  downloads:{route:'downloads',message:'Contains resumable downloads and verification files.'},
  results:{route:'results',message:'New runs and imported report ZIPs are stored here.'}
};
async function chooseSettingsFolder(event){
  const button=event.currentTarget,kind=button.dataset.pickStorage,config=settingsStoragePickers[kind],message=$(`#settings-${kind}-message`),original=button.textContent;
  if(!config)return;
  button.disabled=true;
  button.textContent='Opening…';
  message.classList.remove('error');
  try{
    const response=await fetch(`/api/pick-${config.route}-folder`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}});
    const responseText=await response.text();
    let result={};
    try{
      result=responseText?JSON.parse(responseText):{}
    }catch{
      if(!response.ok)throw new Error(responseText.trim()||`Request failed (${response.status})`);
      throw new Error('The application returned an invalid response')
    }
    if(!response.ok)throw new Error(result.error||`Could not change ${kind} directory`);
    if(result.path){
      await refreshPaths();
      message.textContent=config.message;
      if(kind==='resource')await refreshAppStatus();
      if(kind==='results'){$('#output-folder').value=result.path;await refreshCompletedRuns()}
    }
  }catch(error){
    message.textContent=`Could not change folder: ${error.message}`;
    message.classList.add('error')
  }finally{
    button.disabled=false;
    button.textContent=original
  }
}
function managedResourceIds(){return[...new Set(resourcePlan.resources.filter(resource=>resource.state==='missing').map(resource=>resource.id))]}
function resourceTaskProgress(task,total,completed){
  const parts=[],totalChromosomes=Number(task.totalChromosomes||0);
  if(task.chromosome&&totalChromosomes>0)parts.push(`Chromosome ${task.chromosome} of ${totalChromosomes}`);
  else if(totalChromosomes>0)parts.push(`${task.completedChromosomes||0} of ${totalChromosomes} chromosomes complete`);
  if(total>0)parts.push(`${formatDataSize(completed)} of ${formatDataSize(total)}`);
  else if(completed>0)parts.push(`${formatDataSize(completed)} downloaded`);
  else parts.push(task.phase==='building-cache'?'Building cache':'Preparing');
  if(task.state==='running'&&Number(task.throughputBytesPerSecond)>0)parts.push(`${formatDataSize(task.throughputBytesPerSecond)}/s`);
  return parts.join(' · ')
}
function resourceTaskHtml(task){
  const view=taskJobView(task);
  if(view.kind==='completed')return'';
  const total=Number(task.totalBytes||0),completed=Number(task.completedBytes||0),percent=Math.max(0,Math.min(100,Number(task.percent)||0)),indeterminate=task.state==='running'&&total<=0&&percent<=0,progress=resourceTaskProgress(task,total,completed),showDetail=task.state==='failed'||['reconnecting','retrying'].includes(task.phase),controls=taskActionButtons(task,(task.availableActions||[]).filter(action=>action!=='remove'));
  return`<article class="download-job fui-card" data-download-job="${escapeHtml(task.resourceId)}"><div class="download-job-head"><div><strong class="fui-card__label">${escapeHtml(task.title)}</strong><small class="fui-caption ${taskStateTextClass(view.kind)}">${escapeHtml(view.state)}</small></div><div class="download-job-actions">${controls}</div></div><div class="download-progress-meta fui-caption"><span>${escapeHtml(progress)}</span>${total>0?`<strong>${percent.toFixed(1)}%</strong>`:''}</div><div class="progress-track"${indeterminate?' aria-label="Working"':''}><div class="progress-fill${indeterminate?' indeterminate':''}" style="width:${indeterminate?35:percent}%"></div></div>${showDetail&&view.detail?`<div class="download-detail fui-caption"><span>${escapeHtml(view.detail)}</span></div>`:''}</article>`
}
function applyResourceStatus(id,{download,prepare}){const preparing=prepare.state==='running',prepareQueued=prepare.state==='queued',downloading=download.state==='running',validating=download.state==='validating',queued=download.state==='queued'||prepareQueued,cancelling=download.state==='cancelling',ready=prepare.state==='ready',hasArchive=download.state==='downloaded',failed=download.state==='failed'||prepare.state==='failed',hasPartial=download.downloadedBytes>0&&!hasArchive,hasPreparedPartial=Number(prepare.completedChromosomes||0)>0&&!ready,paused=['paused','cancelled'].includes(download.state)||['paused','cancelled'].includes(prepare.state)||hasPartial||(prepare.state==='idle'&&hasPreparedPartial),hasManagedData=ready||hasArchive||hasPartial||hasPreparedPartial||!['idle','missing'].includes(prepare.state),label=ready?'Installed':cancelling?'Stopping and discarding':preparing?'Installing':validating?'Verifying':failed?'Needs attention':hasArchive&&!paused?'Ready to install':downloading?'Downloading':queued?'Queued':paused?'Paused':'Not installed',busy=preparing||downloading||validating||queued||cancelling,result={download,prepare,ready,label};resourceStates[id]=result;document.querySelectorAll(`[data-resource-state="${id}"]${id==='dbnsfp'?', [data-dbnsfp-state]':''}`).forEach(node=>node.textContent=label);document.querySelectorAll(`[data-resource-review="${id}"]`).forEach(row=>row.classList.toggle('ready',ready));document.querySelectorAll(`[data-resource-storage="${id}"]`).forEach(node=>node.textContent=resourceSize(id,result));document.querySelectorAll(`[data-install="${id}"]`).forEach(button=>{button.disabled=busy;button.innerHTML=`${prototypeIcon(paused?'play':'download')}<span>${paused?'Resume':'Install'}</span>`;button.classList.toggle('resume',paused);button.classList.toggle('install',!paused);button.classList.toggle('hidden',busy||ready)});document.querySelectorAll(`[data-update="${id}"]`).forEach(button=>button.classList.toggle('hidden',!ready||busy));document.querySelectorAll(`[data-source-overflow="${id}"]`).forEach(menu=>{menu.classList.toggle('hidden',!hasManagedData);const trigger=menu.querySelector('[data-source-menu-trigger]');if(trigger)trigger.disabled=busy;if(busy){trigger?.setAttribute('aria-expanded','false');menu.querySelector('.source-action-menu__popover')?.classList.add('hidden')}});document.querySelectorAll(`[data-delete="${id}"]`).forEach(button=>{button.disabled=busy;button.classList.toggle('hidden',!hasManagedData)});return result}
function resourceInstallationBusy(id){const status=resourceStates[id];return['running','validating','queued','cancelling'].includes(status?.download?.state)||['running','queued','cancelling'].includes(status?.prepare?.state)}
async function refreshResourceStatus(id){const[download,prepare]=await Promise.all([fetch(`/api/resources/${id}/download/status`).then(r=>r.json()),fetch(`/api/resources/${id}/prepare/status`).then(r=>r.json())]);return applyResourceStatus(id,{download,prepare})}
async function refreshDownloadStatus(providedSnapshot){if(refreshingResources)return resourceStates;refreshingResources=true;try{normalizeSourceCatalogControls();const ids=managedResourceIds();ids.forEach(ensureDeleteControl);const snapshot=providedSnapshot||await fetch('/api/resources/status').then(async response=>{const body=await response.json();if(!response.ok)throw new Error(body.error||'Resource status unavailable');return body}),entries=ids.map(id=>[id,applyResourceStatus(id,snapshot.resources[id])]),states=Object.fromEntries(entries),setup=snapshot.setup,transcripts=states['ensembl-gff3'];entries.forEach(([id,state])=>applyWizardResourceState(id,state));const coreLabel=setup.ready?'Installed':!setup.referenceReady?'Not installed':transcripts?.prepare.state==='running'?'Installing':transcripts?.download.state==='running'||transcripts?.download.state==='queued'?'Downloading':'Ensembl transcript cache not installed',coreButton=document.querySelector('[data-core-install]'),nextCoreId=setup.referenceReady?'ensembl-gff3':'grch38-reference',nextCore=states[nextCoreId];document.querySelectorAll('[data-reference-state]').forEach(node=>node.textContent=coreLabel);document.querySelector('.required-resource')?.classList.toggle('source-unavailable',!setup.ready);document.querySelectorAll('[data-update="ensembl-gff3"]').forEach(button=>button.classList.toggle('hidden',!setup.ready));if(coreButton){const coreAction=setup.referenceReady?'Install transcripts':'Install';coreButton.classList.toggle('hidden',setup.ready);coreButton.disabled=Boolean(nextCore&&(nextCore.download.state==='running'||nextCore.download.state==='queued'||nextCore.download.state==='cancelling'||nextCore.prepare.state==='running'));coreButton.innerHTML=`${prototypeIcon('download')}<span>${coreAction}</span>`}lastSetupReady=Boolean(setup.ready);document.querySelectorAll('[data-resource-review="core"]').forEach(row=>row.classList.toggle('ready',lastSetupReady));if(currentStep===4){const selectedReady=enabledSourceIds().every(id=>resourceStates[id]?.ready),ready=setup.ready&&selectedReady;$('#continue').disabled=!ready;$('#continue').innerHTML=ready?`${recoveryFiles?'Verify and recover':'Start annotation'} <span>→</span>`:'Install required resources'}updateWizardReadiness();updateReviewResourceStates();updateSetupModal(lastSetupReady);return states}finally{refreshingResources=false}}
async function refreshAppStatus(){const response=await fetch('/api/status'),snapshot=await response.json();if(!response.ok)throw new Error(snapshot.error||'Application status unavailable');await refreshDownloadStatus(snapshot.resources);await refreshAnnotationStatus(snapshot.annotation);await refreshTasks(snapshot.tasks);return snapshot}
async function deleteResource(id){const core=id==='grch38-reference',confirmed=await confirmDestructiveAction({title:`Remove ${resourceTitle(id)} data?`,message:`Downloaded parts and installed data for this source will be deleted.${core?' Local annotation will be unavailable until the shared core package is installed again.':''}`,confirmLabel:'Remove data',cancelLabel:'Keep data'});if(!confirmed)return;const response=await fetch(`/api/resources/${id}/delete`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)showResourceNotice(result.error||'Could not remove resource');await refreshAppStatus()}
function applyWizardResourceState(id,state){document.querySelectorAll(`#wizard-sources input[data-source="${id}"]`).forEach(input=>{input.disabled=!state.ready;if(!state.ready)input.checked=false;else if(selectedProfile()?.sourceIds.includes(id))input.checked=true;input.closest('.source-option')?.classList.toggle('source-unavailable',!state.ready)})}
function sourceOverflowMenuHtml(id){
  const configure=id==='dbnsfp'
    ?`<button type="button" class="fui-menu-item" data-source-menu-config data-dbnsfp-config role="menuitem">${prototypeIcon('sliders-horizontal')}<span>Choose fields</span></button>`
    :configurableSupplementarySources.has(id)
      ?`<button type="button" class="fui-menu-item" data-source-menu-config data-source-fields-config="${escapeHtml(id)}" role="menuitem">${prototypeIcon('sliders-horizontal')}<span>Choose fields</span></button>`
      :'';
  return`<div class="fui-menu source-action-menu hidden" data-source-overflow="${escapeHtml(id)}"><button type="button" class="fui-button fui-button--icon source-action-menu__trigger" data-source-menu-trigger aria-label="More actions" title="More actions" aria-haspopup="menu" aria-expanded="false">${prototypeIcon('more-horizontal')}</button><div class="fui-popover fui-popover--menu fui-menu__popover source-action-menu__popover hidden" role="menu">${configure}<button type="button" class="fui-menu-item fui-menu-item--danger" data-delete="${escapeHtml(id)}" role="menuitem">${prototypeIcon('trash-2')}<span>Remove data</span></button></div></div>`
}
function closeSourceActionMenus(except){
  document.querySelectorAll('[data-source-overflow]').forEach(menu=>{
    if(menu===except)return;
    menu.querySelector('[data-source-menu-trigger]')?.setAttribute('aria-expanded','false');
    menu.querySelector('.source-action-menu__popover')?.classList.add('hidden')
  })
}
function bindSourceActionMenus(){
  document.addEventListener('click',event=>{
    const trigger=event.target.closest('[data-source-menu-trigger]');
    if(trigger){
      const menu=trigger.closest('[data-source-overflow]'),popover=menu?.querySelector('.source-action-menu__popover');
      if(!menu||!popover)return;
      const opening=popover.classList.contains('hidden');
      closeSourceActionMenus(menu);
      popover.classList.toggle('hidden',!opening);
      trigger.setAttribute('aria-expanded',String(opening));
      if(opening)requestAnimationFrame(()=>popover.querySelector('[role="menuitem"]:not(:disabled)')?.focus({preventScroll:true}));
      return
    }
    const menuItem=event.target.closest('[data-source-overflow] [role="menuitem"]');
    if(menuItem){
      closeSourceActionMenus();
      if(menuItem.dataset.delete)void deleteResource(menuItem.dataset.delete);
      return
    }
    if(!event.target.closest('[data-source-overflow]'))closeSourceActionMenus()
  });
  document.addEventListener('keydown',event=>{
    if(event.key!=='Escape')return;
    const menu=document.activeElement?.closest?.('[data-source-overflow]');
    if(!menu)return;
    const trigger=menu.querySelector('[data-source-menu-trigger]');
    closeSourceActionMenus();
    trigger?.focus()
  })
}
function normalizeSourceCatalogControls(){
  if($('#source-list').dataset.normalized)return;
  const required=`<article class="fui-card fui-card--content-compact fui-card--menu-host" data-source-card="grch38-reference"><div class="source-card-copy"><h2 class="fui-card__title fui-card__title-row"><span>Core annotation data</span><small class="fui-badge">Required</small><span class="source-state fui-badge" data-reference-state>Not installed</span></h2><p class="source-card-description fui-card__description">GRCh38 reference and matching transcript cache</p><p class="source-card-storage fui-card__metadata"><strong class="resource-storage" data-resource-storage="grch38-reference">${resourceSize('grch38-reference')}</strong></p></div><div class="source-card-meta"><div class="source-actions"><button type="button" class="fui-button source-state-action hidden" data-update="ensembl-gff3">${prototypeIcon('refresh-cw')}<span>Check for updates</span></button><button class="install fui-button fui-button--primary source-state-action" data-core-install>${prototypeIcon('download')}<span>Install</span></button>${sourceOverflowMenuHtml('grch38-reference')}</div></div></article>`;
  const catalog=orderedCatalogSources().map(source=>{
    const update=`<button type="button" class="fui-button source-state-action hidden" data-update="${escapeHtml(source.id)}">${prototypeIcon('refresh-cw')}<span>Check for updates</span></button>`;
    return`<article class="fui-card fui-card--content-compact fui-card--menu-host" data-source-card="${escapeHtml(source.id)}"><div class="source-card-copy"><h2 class="fui-card__title fui-card__title-row"><span>${escapeHtml(source.name)}</span><span class="source-state fui-badge" data-resource-state="${escapeHtml(source.id)}">Not installed</span></h2><p class="source-card-description fui-card__description">${escapeHtml(source.purpose)}</p><p class="source-card-storage fui-card__metadata"><strong class="resource-storage" data-resource-storage="${escapeHtml(source.id)}">${resourceSize(source.id)}</strong>${sourceLicenseNote(source)}</p></div><div class="source-card-meta"><div class="source-actions">${update}<button class="install fui-button fui-button--primary source-state-action" data-install="${escapeHtml(source.id)}">${prototypeIcon('download')}<span>Install</span></button>${sourceOverflowMenuHtml(source.id)}</div></div></article>`
  }).join('');
  $('#source-list').innerHTML=required+catalog;
  $('#source-list').dataset.normalized='true'
}
function ensureDeleteControl(id){const install=document.querySelector(`#source-list [data-install="${id}"]`);if(!install||document.querySelector(`#source-list [data-delete="${id}"]`))return;install.insertAdjacentHTML('afterend',sourceOverflowMenuHtml(id))}
function installationConcurrency(){const value=Number(localStorage.getItem('annocat.installationConcurrency')||1);return Number.isInteger(value)&&value>=1&&value<=4?value:1}
function sourceInputMode(){const saved=localStorage.getItem('annocat.sourceInputMode');return saved==='pure-streaming'?'pure-streaming':'resumable'}
function setInstallationConcurrency(value){const normalized=Math.min(4,Math.max(1,Number(value)||1));localStorage.setItem('annocat.installationConcurrency',String(normalized));const settings=$('#installation-concurrency');if(settings)settings.value=String(normalized);return normalized}
function setSourceInputMode(value){const normalized=['resumable','hybrid-resumable'].includes(value)?'resumable':'pure-streaming';localStorage.setItem('annocat.sourceInputMode',normalized);const settings=$('#source-input-mode');if(settings)settings.value=normalized;return normalized}
async function requestResourceAction(id,path,update=false){const query=path==='prepare/start'?`?concurrency=${installationConcurrency()}&sourceMode=${sourceInputMode()}${update?'&update=true':''}`:'';const response=await fetch(`/api/resources/${id}/${path}${query}`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not ${path.replace('/',' ')}`);return result}
function showResourceNotice(message){let notice=$('#resource-notice');if(!notice){$('#source-list').insertAdjacentHTML('beforebegin','<div id="resource-notice" class="resource-notice hidden" role="status"></div>');notice=$('#resource-notice')}notice.textContent=message;notice.classList.remove('hidden')}
async function toggleDownloadJob(id,update=false){const status=resourceStates[id],plan=resourcePlan.resources.find(resource=>resource.id===id);if(plan?.installMode==='stream'||status?.download.state==='downloaded'&&!status.ready)await requestResourceAction(id,'prepare/start',update);else await requestResourceAction(id,'download/start')}
async function startConfiguredResourceInstall(id){try{await toggleDownloadJob(id)}catch(error){showResourceNotice(error.message)}finally{await refreshAppStatus()}}
async function handleResourceCardInstall(button){
  const id=button.dataset.install;
  if(button.classList.contains('install')&&(id==='dbnsfp'||configurableSupplementarySources.has(id))){
    if(id==='dbnsfp')await showDbnsfpFieldConfiguration({installAfterSave:true});
    else await showSupplementaryFieldConfiguration(id,{installAfterSave:true});
    return
  }
  await handleDownloadJobAction(id,'resume',button)
}
async function startCoreInstall(){const snapshot=await fetch('/api/resources/status').then(r=>r.json()),failures=[];for(const id of['grch38-reference','ensembl-gff3']){const status=snapshot.resources[id],ready=status.prepare.state==='ready',busy=['running','validating','queued','cancelling'].includes(status.download.state)||status.prepare.state==='running';if(ready||busy)continue;try{if(status.download.state==='downloaded')await requestResourceAction(id,'prepare/start');else await requestResourceAction(id,'download/start')}catch(error){failures.push(`${resourceTitle(id)}: ${error.message}`)}}if(failures.length)showResourceNotice(`Core annotation setup could not start: ${failures.join(' · ')}`);await refreshAppStatus()}
async function handleDownloadJobAction(id,action,button){if(action==='remove'){await deleteResource(id);return}if(action==='cancel'&&!await confirmDestructiveAction({title:`Cancel ${resourceTitle(id)} installation?`,message:'The task will stop and its downloaded and partially installed data will be discarded. Other installed data sources are not affected.',confirmLabel:'Cancel installation',cancelLabel:'Keep running'}))return;button.disabled=true;if(['resume','install'].includes(action))button.classList.add('hidden');try{if(action==='pause')await requestResourceAction(id,'download/pause');else if(action==='cancel')await requestResourceAction(id,'download/cancel');else await toggleDownloadJob(id)}catch(error){showResourceNotice(error.message)}finally{button.disabled=false;await refreshAppStatus()}}
async function checkSourceUpdate(id,button){
  const original=button.innerHTML;
  let feedback='';
  button.disabled=true;
  button.setAttribute('aria-busy','true');
  button.innerHTML=`${prototypeIcon('refresh-cw')}<span>Checking…</span>`;
  try{
    const response=await fetch(`/api/resources/${id}/updates/check`),result=await response.json();
    if(!response.ok)throw new Error(result.error||'Update check failed');
    if(!result.installed){
      feedback='Update available';
      showResourceNotice(`${resourceTitle(id)} is not installed. Available version: ${result.currentVersion}.`)
    }else if(result.updateAvailable){
      feedback='Update available';
      if(confirm(`${resourceTitle(id)} ${result.currentVersion} is available.\n\nInstall it alongside ${result.installedVersions.join(', ')}?`))await toggleDownloadJob(id,true)
    }else{
      feedback='Up to date';
      showResourceNotice(`${resourceTitle(id)} is up to date (${result.currentVersion}).`)
    }
  }catch(error){
    feedback='Check failed';
    showResourceNotice(error.message)
  }finally{
    button.disabled=false;
    button.removeAttribute('aria-busy');
    button.innerHTML=`${prototypeIcon('refresh-cw')}<span>${feedback||'Check complete'}</span>`;
    setTimeout(()=>{if(button.isConnected&&!button.hasAttribute('aria-busy'))button.innerHTML=original},2500);
    await refreshAppStatus()
  }
}
function installableProfileResources(profile){const requested=new Set(['grch38-reference','ensembl-gff3',...(profile?.sourceIds||[])]);return resourcePlan.resources.filter(resource=>requested.has(resource.id)&&resource.state==='missing'&&!resourceStates[resource.id]?.ready)}
const dbnsfpCoordinateFields=new Set(['chr','pos(1-based)','ref','alt']);
async function loadDbnsfpConfiguration(){const response=await fetch('/api/resources/dbnsfp/config'),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not load dbNSFP field configuration');dbnsfpConfiguration=result;return result}
const dbnsfpFieldDetails={aaref:['Reference amino acid','Original amino acid for this protein change.'],aaalt:['Alternate amino acid','Substituted amino acid for this protein change.'],aapos:['Protein position','Amino-acid position within the protein.'],genename:['Gene symbol','Human-readable gene symbol associated with the prediction.'],Ensembl_geneid:['Ensembl gene ID','Stable Ensembl identifier for the gene.'],Ensembl_transcriptid:['Ensembl transcript ID','Transcript to which transcript-specific scores apply.'],Ensembl_proteinid:['Ensembl protein ID','Protein product associated with the transcript.'],Uniprot_acc:['UniProt accession','UniProt protein identifier.'],HGVSc_VEP:['Coding HGVS','VEP coding/transcript HGVS description.'],HGVSp_VEP:['Protein HGVS','VEP protein HGVS description.'],APPRIS:['APPRIS annotation','Principal or alternative transcript classification.'],GENCODE_basic:['GENCODE Basic','Whether the transcript belongs to the GENCODE Basic subset.'],TSL:['Transcript support level','Evidence-based Ensembl transcript support level.'],VEP_canonical:['Canonical transcript','Whether Ensembl marks this transcript as canonical.'],SIFT_score:['SIFT score','Missense tolerance score; lower values indicate a more damaging prediction.'],SIFT_pred:['SIFT prediction','Categorical tolerated or deleterious SIFT result.'],Polyphen2_HDIV_score:['PolyPhen-2 HDIV score','Missense damaging score trained for rare Mendelian disease variants.'],Polyphen2_HDIV_pred:['PolyPhen-2 HDIV prediction','Benign, possibly damaging, or probably damaging category.'],Polyphen2_HVAR_score:['PolyPhen-2 HVAR score','Missense damaging score trained on a broader disease-variant set.'],Polyphen2_HVAR_pred:['PolyPhen-2 HVAR prediction','Categorical PolyPhen-2 HVAR result.'],REVEL_score:['REVEL score','Missense ensemble score from 0 to 1; higher values indicate greater predicted pathogenicity.'],AlphaMissense_score:['AlphaMissense score','Deep-learning missense pathogenicity score.'],AlphaMissense_pred:['AlphaMissense prediction','Categorical benign, uncertain, or pathogenic prediction.'],PrimateAI_score:['PrimateAI score','Missense pathogenicity score informed by primate variation.'],PrimateAI_pred:['PrimateAI prediction','Categorical PrimateAI missense prediction.'],CADD_raw:['Raw CADD score','Unscaled CADD model score.'],CADD_phred:['CADD PHRED score','Rank-scaled deleteriousness score; higher values indicate stronger predicted impact.'],'GERP++_RS':['GERP++ rejected substitutions','Evolutionary constraint score; higher positive values indicate stronger conservation.'],phyloP100way_vertebrate:['phyloP 100-way score','Vertebrate base-level conservation score.'],Interpro_domain:['InterPro domain','Protein domain or functional site containing the amino-acid change.']};
dbnsfpFieldDetails.sift=['SIFT prediction','Legacy combined SIFT category and score; lower scores indicate a more damaging missense prediction.'];
dbnsfpFieldDetails.polyphen=['PolyPhen prediction','Legacy combined PolyPhen-2 category and score for predicted missense impact.'];
function readableFieldName(field){return field.replace(/_/g,' ').replace(/([a-z])([A-Z])/g,'$1 $2').replace(/\b[a-z]/g,letter=>letter.toUpperCase())}
function dbnsfpFieldPresentation(field){if(dbnsfpFieldDetails[field])return dbnsfpFieldDetails[field];const method=field.replace(/_(converted_)?rankscore$|_score$|_pred$/,'').replace(/_/g,' ');if(/rankscore$/.test(field))return[`${readableFieldName(method)} rank score`,`Percentile-like ranking of the ${method} result within dbNSFP; useful for comparing variants.`];if(/_score$/.test(field))return[readableFieldName(field),`Numeric prediction score reported by ${method}. Direction and recommended thresholds depend on that method.`];if(/_pred$/.test(field))return[readableFieldName(field),`Categorical prediction reported by ${method}, such as damaging or tolerated.`];return[readableFieldName(field),'Transcript-linked annotation retained from dbNSFP 4.9a.']}
function dbnsfpEditorHtml(configuration){const selected=new Set(configuration.selection.fields),groups=configuration.contract.groups.map(group=>{const payload=group.fields.filter(field=>!dbnsfpCoordinateFields.has(field)),required=Boolean(group.required),checked=payload.filter(field=>selected.has(field)).length;return`<section class="dbnsfp-field-group fui-field-config__group" data-dbnsfp-field-group><div class="dbnsfp-group-heading fui-field-config__group-heading"><label><input class="fui-checkbox" type="checkbox" data-dbnsfp-group ${required||checked===payload.length?'checked':''} ${required||configuration.locked?'disabled':''}><span><strong>${escapeHtml(group.label||group.id)}</strong><small>${required?'Required for variant and transcript matching':`${checked} of ${payload.length} retained`}</small></span></label></div><div class="dbnsfp-field-list source-field-list fui-field-config__grid">${payload.map(field=>{const[label,description]=dbnsfpFieldPresentation(field);return`<label class="fui-field-config__option" title="${escapeHtml(field)}"><input class="fui-checkbox" type="checkbox" data-dbnsfp-field="${escapeHtml(field)}" ${required||selected.has(field)?'checked':''} ${required||configuration.locked?'disabled':''}><span class="source-field-copy fui-field-config__copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(description)}</small><code>${escapeHtml(field)}</code></span></label>`}).join('')}</div></section>`}).join('');return`<div class="dbnsfp-field-editor fui-field-config" data-dbnsfp-editor><div class="dbnsfp-editor-head fui-field-config__header"><div><strong>dbNSFP retained fields</strong><small data-dbnsfp-field-count></small></div>${configuration.locked?'<span class="field-lock fui-badge">Prepared cache</span>':'<div><button type="button" class="fui-button" data-dbnsfp-recommended>Recommended</button></div>'}</div><p>Recommended keeps transcript-linked SIFT, PolyPhen, REVEL, AlphaMissense, PrimateAI, and GERP++ fields. Dedicated CADD, phyloP, gnomAD, ClinVar, dbSNP, and SpliceAI sources remain independently namespaced.</p>${groups}${configuration.locked?'<p class="dbnsfp-locked-note fui-status-message fui-status-message--warning">This prepared dbNSFP cache already uses this field set. Remove the cache before changing it.</p>':''}</div>`}
function updateDbnsfpEditor(editor){const fields=[...editor.querySelectorAll('[data-dbnsfp-field]')],selected=fields.filter(field=>field.checked);editor.querySelector('[data-dbnsfp-field-count]').textContent=`${selected.length} of ${fields.length} cache fields retained`;editor.querySelectorAll('[data-dbnsfp-field-group]').forEach(group=>{const checkbox=group.querySelector('[data-dbnsfp-group]'),items=[...group.querySelectorAll('[data-dbnsfp-field]')];if(!checkbox||checkbox.disabled)return;checkbox.checked=items.every(item=>item.checked);checkbox.indeterminate=!checkbox.checked&&items.some(item=>item.checked);group.querySelector('small').textContent=`${items.filter(item=>item.checked).length} of ${items.length} retained`})}
function bindDbnsfpEditor(editor){if(!editor)return;editor.addEventListener('change',event=>{if(event.target.matches('[data-dbnsfp-group]'))editor.querySelectorAll(`[data-dbnsfp-field-group]`).forEach(group=>{if(group.contains(event.target))group.querySelectorAll('[data-dbnsfp-field]:not(:disabled)').forEach(field=>field.checked=event.target.checked)});updateDbnsfpEditor(editor)});const recommended=new Set(dbnsfpConfiguration?.contract?.recommendedFields||[]);editor.querySelector('[data-dbnsfp-recommended]')?.addEventListener('click',()=>{editor.querySelectorAll('[data-dbnsfp-field]:not(:disabled)').forEach(field=>field.checked=recommended.has(field.dataset.dbnsfpField));updateDbnsfpEditor(editor)});updateDbnsfpEditor(editor)}
function sameFieldSelection(left,right){if(left.length!==right.length)return false;const expected=new Set(right);return expected.size===right.length&&left.every(field=>expected.has(field))}
async function saveDbnsfpEditor(editor){if(!editor||dbnsfpConfiguration?.locked)return dbnsfpConfiguration;const required=dbnsfpConfiguration.contract.groups.filter(group=>group.required).flatMap(group=>group.fields).filter(field=>!dbnsfpCoordinateFields.has(field)),checked=[...editor.querySelectorAll('[data-dbnsfp-field]:checked')].map(input=>input.dataset.dbnsfpField),fields=[...new Set([...required,...checked])];if(sameFieldSelection(fields,dbnsfpConfiguration.selection.fields))return dbnsfpConfiguration;const response=await fetch('/api/resources/dbnsfp/config',{method:'POST',headers:{'X-AnnoCat-CSRF':'1','Content-Type':'application/json'},body:JSON.stringify({schemaVersion:dbnsfpConfiguration.selection.schemaVersion,contractId:dbnsfpConfiguration.selection.contractId,fields})}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not save dbNSFP field configuration');dbnsfpConfiguration=result;return result}
async function showDbnsfpFieldConfiguration({installAfterSave=false}={}){
  try{
    const configuration=await loadDbnsfpConfiguration();
    let dialog=$('#dbnsfp-field-dialog');
    if(!dialog){
      document.body.insertAdjacentHTML('beforeend','<dialog id="dbnsfp-field-dialog" class="dbnsfp-config-dialog fui-dialog fui-dialog--wide" tabindex="-1" aria-labelledby="dbnsfp-field-dialog-title"><form class="fui-dialog__surface"><header class="fui-dialog__header"><div><p class="kicker">DBNSFP 4.9A</p><h2 id="dbnsfp-field-dialog-title">Configure retained fields</h2></div></header><div class="fui-dialog__content fui-dialog__content--scrollable" data-dbnsfp-dialog-editor></div><footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="button" class="fui-button" data-dbnsfp-close>Cancel</button><button type="button" class="fui-button fui-button--primary" data-dbnsfp-save>Save fields</button></div></footer></form></dialog>');
      dialog=$('#dbnsfp-field-dialog');
      dialog.querySelector('[data-dbnsfp-close]').addEventListener('click',()=>dialog.close());
      dialog.querySelector('[data-dbnsfp-save]').addEventListener('click',async event=>{
        event.currentTarget.disabled=true;
        try{
          await saveDbnsfpEditor(dialog.querySelector('[data-dbnsfp-editor]'));
          const startInstall=dialog.dataset.installAfterSave==='true';
          dialog.close();
          if(startInstall)await startConfiguredResourceInstall('dbnsfp')
        }catch(error){
          showResourceNotice(error.message)
        }finally{
          event.currentTarget.disabled=false
        }
      })
    }
    dialog.dataset.installAfterSave=String(installAfterSave);
    dialog.querySelector('[data-dbnsfp-dialog-editor]').innerHTML=dbnsfpEditorHtml(configuration);
    bindDbnsfpEditor(dialog.querySelector('[data-dbnsfp-editor]'));
    const save=dialog.querySelector('[data-dbnsfp-save]');
    save.textContent=installAfterSave?'Save and install':'Save fields';
    save.classList.toggle('hidden',configuration.locked);
    openFluentDialog(dialog);
    return dialog
  }catch(error){
    showResourceNotice(error.message);
    return null
  }
}
const configurableSupplementarySources=new Set(['clinvar','dbsnp','gnomad','gnomad-genomes','phylop','cadd','spliceai','revel']);
const supplementaryFieldDetails={
clinvar:{significance:['ClinVar classification','ClinVar classification such as pathogenic, benign, or uncertain significance.'],reviewStatus:['Review status','Confidence and review level behind the ClinVar assertion.'],phenotypes:['Conditions','Diseases or traits associated with the submitted variant.'],variantClass:['Variant class','ClinVar category such as SNV, deletion, insertion, or duplication.'],soAccession:['Sequence Ontology ID','Standard Sequence Ontology identifier for the variant class.'],afExac:['ExAC allele frequency','Population allele frequency reported by ExAC.'],afTgp:['1000 Genomes allele frequency','Population allele frequency reported by the 1000 Genomes Project.'],afEsp:['ESP allele frequency','Population allele frequency reported by the NHLBI Exome Sequencing Project.'],geneInfo:['Gene information','ClinVar gene symbols and identifiers linked to the variant.'],diseaseDatabases:['Disease database links','Identifiers connecting the assertion to disease databases.'],molecularConsequences:['Molecular consequences','ClinVar molecular consequence terms for the allele.'],origin:['Allele origin','Reported origin such as germline, somatic, inherited, or de novo.'],conflictingSignificance:['Conflicting classifications','Clinical significance values when submitters disagree.']},
dbsnp:{id:['Reference SNP ID','The rs identifier used to reference this variant in dbSNP.'],globalMaf:['Global minor-allele frequency','Legacy global minor-allele frequency carried by dbSNP.'],variantType:['Variant type','dbSNP type such as SNV, insertion, deletion, or microsatellite.'],common:['Common-variant flag','Whether dbSNP marks the allele as common in population data.']},
gnomad:{allAf:['Overall allele frequency','Alternate-allele frequency across all represented gnomAD samples.'],allAn:['Allele number','Number of chromosomes with usable genotype data.'],allAc:['Allele count','Number of observed alternate alleles.'],allHc:['Homozygote count','Number of individuals homozygous for the alternate allele.'],afrAf:['African/African American AF','Allele frequency in the African/African American group.'],amrAf:['Admixed American AF','Allele frequency in the Admixed American group.'],asjAf:['Ashkenazi Jewish AF','Allele frequency in the Ashkenazi Jewish group.'],easAf:['East Asian AF','Allele frequency in the East Asian group.'],finAf:['Finnish AF','Allele frequency in the Finnish group.'],midAf:['Middle Eastern AF','Allele frequency in the Middle Eastern group.'],nfeAf:['Non-Finnish European AF','Allele frequency in the non-Finnish European group.'],othAf:['Other ancestry AF','Allele frequency in samples assigned to the other group.'],remainingAf:['Remaining ancestry AF','Allele frequency in samples outside the named groups.'],sasAf:['South Asian AF','Allele frequency in the South Asian group.'],filters:['Quality filters','gnomAD site filters; PASS means the site passed release quality checks.'],faf95:['Filtering AF (95%)','Conservative 95% filtering allele frequency for rarity assessment.'],faf99:['Filtering AF (99%)','More conservative 99% filtering allele frequency for rarity assessment.'],grpmaxAf:['Maximum group AF','Highest ancestry-group allele frequency for the variant.'],grpmaxPopulation:['Maximum-frequency group','Ancestry group in which the highest frequency was observed.']},
phylop:{score:['phyloP 100-way score','Base-level evolutionary conservation; positive values indicate conservation and negative values acceleration.'],value:['phyloP 100-way score','Base-level evolutionary conservation; positive values indicate conservation and negative values acceleration.']},
cadd:{raw:['Raw CADD score','Unscaled model score useful for comparisons within the same CADD release.'],phred:['CADD PHRED score','Rank-scaled deleteriousness score; higher values indicate stronger predicted impact.']},
spliceai:{gene:['Gene symbol','Gene associated with this SpliceAI prediction.'],maxDeltaScore:['Maximum delta score','Maximum of the acceptor-gain, acceptor-loss, donor-gain, and donor-loss scores for this allele.'],dsAg:['Acceptor gain score','Probability-like delta score for creating an acceptor site.'],dsAl:['Acceptor loss score','Probability-like delta score for losing an acceptor site.'],dsDg:['Donor gain score','Probability-like delta score for creating a donor site.'],dsDl:['Donor loss score','Probability-like delta score for losing a donor site.'],dpAg:['Acceptor gain position','Predicted position offset for acceptor gain.'],dpAl:['Acceptor loss position','Predicted position offset for acceptor loss.'],dpDg:['Donor gain position','Predicted position offset for donor gain.'],dpDl:['Donor loss position','Predicted position offset for donor loss.']},
revel:{score:['REVEL score','Missense pathogenicity ensemble score from 0 to 1; higher values indicate greater predicted pathogenicity.'],transcriptId:['Transcript ID','Ensembl transcript to which the REVEL score applies.'],aaRef:['Reference amino acid','Original amino acid used for the transcript-level prediction.'],aaAlt:['Alternate amino acid','Substituted amino acid used for the transcript-level prediction.']}
};
supplementaryFieldDetails['gnomad-genomes']=supplementaryFieldDetails.gnomad;
function supplementaryFieldPresentation(resourceId,field){return supplementaryFieldDetails[resourceId]?.[field]||[field,'Retained exactly as emitted by the pinned fastVEP parser.']}
function evidenceFieldPresentationBase(field){const source=String(field?.sourceId||'').toLowerCase(),path=String(field?.fieldPath||''),leaf=path.split(/[.\[\]]/).filter(Boolean).pop()||path;let details;if(source.includes('dbnsfp'))details=dbnsfpFieldPresentation(leaf);else if(source.includes('favor'))details=favorFieldPresentation(path,leaf);else{const resourceId=['gnomad-genomes','clinvar','dbsnp','gnomad','phylop','cadd','spliceai','revel'].find(id=>source===id||source.startsWith(`${id}-`)||source.startsWith(`${id}@`));details=resourceId?supplementaryFieldPresentation(resourceId,leaf):[readableFieldName(leaf||'Evidence field'),'Annotation field discovered in this report.']}return{label:details[0],description:details[1],fieldPath:path,sourceId:field?.sourceId||'',valueType:field?.valueType||'unknown'}}
async function loadSupplementaryFieldConfiguration(resourceId){const response=await fetch(`/api/resources/${encodeURIComponent(resourceId)}/fields`),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not load ${resourceTitle(resourceId)} field configuration`);supplementaryFieldConfigurations.set(resourceId,result);return result}
function supplementaryFieldEditorHtml(resourceId,configuration){const selected=new Set(configuration.selection.fields),groups=configuration.contract.groups.map(group=>{const checked=group.fields.filter(field=>selected.has(field)).length,required=Boolean(group.required);return`<section class="dbnsfp-field-group fui-field-config__group" data-source-field-group><div class="dbnsfp-group-heading fui-field-config__group-heading"><label><input class="fui-checkbox" type="checkbox" data-source-field-group-toggle ${required||checked===group.fields.length?'checked':''} ${required||configuration.locked?'disabled':''}><span><strong>${escapeHtml(group.label||group.id)}</strong><small>${required?'Required':`${checked} of ${group.fields.length} retained`}</small></span></label></div><div class="dbnsfp-field-list source-field-list fui-field-config__grid">${group.fields.map(field=>{const[label,description]=supplementaryFieldPresentation(resourceId,field);return`<label class="fui-field-config__option" title="${escapeHtml(field)}"><input class="fui-checkbox" type="checkbox" data-source-field="${escapeHtml(field)}" ${required||selected.has(field)?'checked':''} ${required||configuration.locked?'disabled':''}><span class="source-field-copy fui-field-config__copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(description)}</small><code>${escapeHtml(field)}</code></span></label>`}).join('')}</div></section>`}).join('');const fullByDefault=resourceId==='gnomad'||resourceId==='gnomad-genomes';return`<div class="dbnsfp-field-editor fui-field-config" data-source-field-editor="${escapeHtml(resourceId)}"><div class="dbnsfp-editor-head fui-field-config__header"><div><strong>${escapeHtml(resourceTitle(resourceId))} retained fields</strong><small data-source-field-count></small></div>${configuration.locked?'<span class="field-lock fui-badge">Prepared cache</span>':`<button type="button" class="fui-button" data-source-field-defaults>${fullByDefault?'Select all':'Restore defaults'}</button>`}</div><p>Choose what AnnoCat keeps in the local fastVEP cache. Keeping fewer fields reduces cache size. Every choice below is supported by the bundled parser and appears under this source in results.</p>${groups}${configuration.locked?`<p class="dbnsfp-locked-note fui-status-message fui-status-message--warning">This prepared ${escapeHtml(resourceTitle(resourceId))} cache already uses this field set. Remove the cache before changing it.</p>`:''}</div>`}
function updateSupplementaryFieldEditor(editor){if(!editor)return;const checked=editor.querySelectorAll('[data-source-field]:checked').length,total=editor.querySelectorAll('[data-source-field]').length;editor.querySelector('[data-source-field-count]').textContent=`${checked} of ${total} fields retained`;editor.querySelectorAll('[data-source-field-group]').forEach(group=>{const toggle=group.querySelector('[data-source-field-group-toggle]'),fields=[...group.querySelectorAll('[data-source-field]')],selected=fields.filter(field=>field.checked).length;if(toggle&&!toggle.disabled){toggle.checked=fields.length>0&&selected===fields.length;toggle.indeterminate=selected>0&&!toggle.checked;group.querySelector('small').textContent=`${selected} of ${fields.length} retained`}})}
function bindSupplementaryFieldEditor(editor){if(!editor)return;const resourceId=editor.dataset.sourceFieldEditor,configuration=supplementaryFieldConfigurations.get(resourceId),fullByDefault=resourceId==='gnomad'||resourceId==='gnomad-genomes';editor.addEventListener('change',event=>{if(event.target.matches('[data-source-field-group-toggle]'))event.target.closest('[data-source-field-group]').querySelectorAll('[data-source-field]:not(:disabled)').forEach(field=>field.checked=event.target.checked);updateSupplementaryFieldEditor(editor)});const defaults=new Set(configuration.contract.groups.filter(group=>fullByDefault||group.default||group.required).flatMap(group=>group.fields));editor.querySelector('[data-source-field-defaults]')?.addEventListener('click',()=>{editor.querySelectorAll('[data-source-field]:not(:disabled)').forEach(field=>field.checked=defaults.has(field.dataset.sourceField));updateSupplementaryFieldEditor(editor)});updateSupplementaryFieldEditor(editor)}
async function saveSupplementaryFieldEditor(editor){if(!editor)return;const resourceId=editor.dataset.sourceFieldEditor,configuration=supplementaryFieldConfigurations.get(resourceId);if(configuration?.locked)return configuration;const fields=[...editor.querySelectorAll('[data-source-field]:checked')].map(input=>input.dataset.sourceField);if(!fields.length)throw new Error(`Select at least one ${resourceTitle(resourceId)} field`);if(sameFieldSelection(fields,configuration.selection.fields))return configuration;const response=await fetch(`/api/resources/${encodeURIComponent(resourceId)}/fields`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1','Content-Type':'application/json'},body:JSON.stringify({schemaVersion:configuration.selection.schemaVersion,contractId:configuration.selection.contractId,fields})}),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not save ${resourceTitle(resourceId)} fields`);supplementaryFieldConfigurations.set(resourceId,result);return result}
async function showSupplementaryFieldConfiguration(resourceId,{installAfterSave=false}={}){
  try{
    const configuration=await loadSupplementaryFieldConfiguration(resourceId);
    let dialog=$('#supplementary-field-dialog');
    if(!dialog){
      document.body.insertAdjacentHTML('beforeend','<dialog id="supplementary-field-dialog" class="dbnsfp-config-dialog fui-dialog fui-dialog--wide" tabindex="-1" aria-labelledby="supplementary-field-dialog-title"><form class="fui-dialog__surface"><header class="fui-dialog__header"><div><p class="kicker" data-source-field-kicker></p><h2 id="supplementary-field-dialog-title">Configure retained fields</h2></div></header><div class="fui-dialog__content fui-dialog__content--scrollable" data-source-field-dialog-editor></div><footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="button" class="fui-button" data-source-field-close>Cancel</button><button type="button" class="fui-button fui-button--primary" data-source-field-save>Save fields</button></div></footer></form></dialog>');
      dialog=$('#supplementary-field-dialog');
      dialog.querySelector('[data-source-field-close]').addEventListener('click',()=>dialog.close());
      dialog.querySelector('[data-source-field-save]').addEventListener('click',async event=>{
        event.currentTarget.disabled=true;
        try{
          await saveSupplementaryFieldEditor(dialog.querySelector('[data-source-field-editor]'));
          const installId=dialog.dataset.installAfterSave;
          dialog.close();
          if(installId)await startConfiguredResourceInstall(installId)
        }catch(error){
          showResourceNotice(error.message)
        }finally{
          event.currentTarget.disabled=false
        }
      })
    }
    dialog.dataset.installAfterSave=installAfterSave?resourceId:'';
    dialog.querySelector('[data-source-field-kicker]').textContent=resourceTitle(resourceId);
    dialog.querySelector('[data-source-field-dialog-editor]').innerHTML=supplementaryFieldEditorHtml(resourceId,configuration);
    bindSupplementaryFieldEditor(dialog.querySelector('[data-source-field-editor]'));
    const save=dialog.querySelector('[data-source-field-save]');
    save.disabled=false;
    save.textContent=installAfterSave?'Save and install':'Save fields';
    save.classList.toggle('hidden',configuration.locked);
    openFluentDialog(dialog);
    return dialog
  }catch(error){
    showResourceNotice(error.message);
    return null
  }
}
function profileReviewResources(profile,installable){const items=[...installable],seen=new Set(items.map(item=>item.id));for(const id of profile.sourceIds||[]){if(seen.has(id)||!resourceStates[id]?.ready||!(id==='dbnsfp'||configurableSupplementarySources.has(id)))continue;const item=resourcePlan.resources.find(resource=>resource.id===id);if(item){items.push(item);seen.add(id)}}return items}
function profileInstallRow(title,size,action=''){return`<div class="install-review-row fui-list-row fui-list-row--action"><strong>${escapeHtml(title)}</strong><span>${escapeHtml(size)}</span><div class="fui-list-row__actions">${action}</div></div>`}
async function profileInstallItemsHtml(items){const sections=[],coreItems=items.filter(item=>coreResourceIds.has(item.id));if(coreItems.length){const networkBytes=coreItems.reduce((sum,item)=>sum+Number(item.downloadBytes||0),0),size=networkBytes?`${formatDataSize(networkBytes)} download`:'Size determined during install';sections.push(profileInstallRow('Core annotation data',size))}for(const item of items.filter(item=>!coreResourceIds.has(item.id))){let configuration=null;if(item.id==='dbnsfp')configuration=await loadDbnsfpConfiguration();else if(configurableSupplementarySources.has(item.id))configuration=await loadSupplementaryFieldConfiguration(item.id);const size=profileInstallSize(item);if(!configuration){sections.push(profileInstallRow(resourceTitle(item.id),size));continue}const fieldCount=configuration.selection.fields.length,label=configuration.locked?'View fields':'Edit fields',action=`<small class="fui-caption">${fieldCount} field${fieldCount===1?'':'s'}</small><button type="button" class="fui-button fui-button--small" data-profile-install-fields="${escapeHtml(item.id)}">${label}</button>`;sections.push(profileInstallRow(resourceTitle(item.id),size,action))}return sections.join('')}
function updateProfileInstallRuntimeCopy(dialog){const count=installationConcurrency(),mode=sourceInputMode(),installable=Number(dialog.dataset.installableCount||0),total=Number(dialog.dataset.installNetworkBytes||0),unknownSizes=Number(dialog.dataset.installUnknownSizes||0),readyCache=Number(dialog.dataset.readyCacheBytes||0),network=unknownSizes?`${total?`${formatDataSize(total)} known download · `:''}${unknownSizes} size${unknownSizes===1?'':'s'} determined during install`:`${formatDataSize(total)} download`,concurrency=count===1?'one source at a time':`up to ${count} sources at a time`;dialog.querySelector('[data-install-summary]').textContent=`${installable} source${installable===1?'':'s'} · ${network} · ${concurrency}${readyCache?` · ${formatDataSize(readyCache)} already on disk`:''}`;dialog.querySelector('[data-install-stream-note]').textContent=mode==='pure-streaming'?'Streaming uses less disk, but an interruption may restart the current source part.':'Resumable saves a temporary part so interrupted downloads continue without replay.'}
async function editProfileInstallFields(button){const review=button.closest('#profile-install-review'),profileId=review?.dataset.profileId,resourceId=button.dataset.profileInstallFields;if(!review||!profileId||!resourceId)return;review.close();const editor=resourceId==='dbnsfp'?await showDbnsfpFieldConfiguration():await showSupplementaryFieldConfiguration(resourceId);if(!editor){await showProfileInstallReview(profileId);return}editor.addEventListener('close',()=>showProfileInstallReview(profileId),{once:true})}
async function showProfileInstallReview(profileId){
  const profile=profiles.find(item=>item.id===profileId);
  if(!profile)return;
  const installable=installableProfileResources(profile),reviewItems=profileReviewResources(profile,installable),installableIds=new Set(installable.map(item=>item.id)),pending=(profile.sourceIds||[]).filter(id=>id!=='fastvep'&&!installableIds.has(id)&&!resourceStates[id]?.ready).map(id=>sources.find(source=>source.id===id)?.name||id),total=installable.reduce((sum,item)=>sum+(item.downloadBytes||0),0),unknownSizes=installable.filter(item=>!item.downloadBytes).length,readyCache=(profile.sourceIds||[]).reduce((sum,id)=>sum+Number(resourceStates[id]?.prepare?.preparedBytes||0),0);
  let installItemsHtml='';
  try{installItemsHtml=await profileInstallItemsHtml(reviewItems)}catch(error){showResourceNotice(error.message);return}
  let dialog=$('#profile-install-review');
  if(!dialog){
    document.body.insertAdjacentHTML('beforeend','<dialog id="profile-install-review" class="profile-install-review fui-dialog fui-dialog--wide" tabindex="-1" aria-labelledby="profile-install-review-title"><form class="fui-dialog__surface"><header class="fui-dialog__header"><div><p class="kicker">Recommended profile</p><h2 id="profile-install-review-title" data-install-title></h2><p class="fui-dialog__description" data-install-summary></p></div></header><div class="fui-dialog__content fui-dialog__content--scrollable"><div class="fui-list fui-list--divided" data-install-items></div><p class="install-pending fui-caption" data-install-pending></p><section class="fui-form-section"><div class="fui-form-section__header"><h3>Download settings</h3><p>These settings apply to this installation.</p></div><div class="install-runtime-options fui-form-grid"><label class="fui-field"><span class="fui-field__label">Download safety</span><select class="fui-select" data-install-source-mode><option value="resumable">Resumable — Recommended</option><option value="pure-streaming">Pure streaming — Uses less temporary disk</option></select></label><label class="fui-field"><span class="fui-field__label">Concurrent installs</span><select class="fui-select" data-install-concurrency><option value="1">1 — Recommended</option><option value="2">2 — Faster</option><option value="3">3 — High resource use</option><option value="4">4 — Maximum resource use</option></select></label></div><p class="install-stream-note fui-caption" data-install-stream-note></p></section></div><footer class="fui-dialog__footer"><div class="fui-dialog__actions"><button type="button" class="fui-button" data-close-profile-install>Cancel</button><button type="button" class="fui-button fui-button--primary" data-confirm-profile-install>Install sources</button></div></footer></form></dialog>');
    dialog=$('#profile-install-review');
    dialog.querySelector('[data-close-profile-install]').addEventListener('click',()=>dialog.close());
    dialog.querySelector('[data-confirm-profile-install]').addEventListener('click',event=>queueProfileInstall(event.currentTarget.dataset.confirmProfileInstall,dialog));
    dialog.querySelector('[data-install-source-mode]').addEventListener('change',event=>{setSourceInputMode(event.target.value);updateProfileInstallRuntimeCopy(dialog)});
    dialog.querySelector('[data-install-concurrency]').addEventListener('change',event=>{setInstallationConcurrency(event.target.value);updateProfileInstallRuntimeCopy(dialog)});
  }
  dialog.dataset.installableCount=String(installable.length);
  dialog.dataset.profileId=profileId;
  dialog.dataset.installNetworkBytes=String(total);
  dialog.dataset.installUnknownSizes=String(unknownSizes);
  dialog.dataset.readyCacheBytes=String(readyCache);
  dialog.querySelector('[data-install-title]').textContent=`Install ${profile.name}`;
  dialog.querySelector('[data-install-source-mode]').value=sourceInputMode();
  dialog.querySelector('[data-install-concurrency]').value=String(installationConcurrency());
  updateProfileInstallRuntimeCopy(dialog);
  dialog.querySelector('[data-install-items]').innerHTML=installItemsHtml;
  const pendingMessage=dialog.querySelector('[data-install-pending]');
  pendingMessage.textContent=pending.length?`${pending.join(', ')} are still pending verified installers or catalogs and will not be started.`:'';
  pendingMessage.classList.toggle('hidden',pending.length===0);
  const confirmButton=dialog.querySelector('[data-confirm-profile-install]');
  confirmButton.dataset.confirmProfileInstall=profileId;
  confirmButton.disabled=installable.length===0;
  openFluentDialog(dialog);
}
async function queueProfileInstall(profileId,dialog){const profile=profiles.find(item=>item.id===profileId),resources=installableProfileResources(profile),failures=[],button=dialog?.querySelector('[data-confirm-profile-install]');if(button)button.disabled=true;try{for(const resource of resources.filter(item=>item.installMode!=='stream')){const status=await refreshResourceStatus(resource.id);if(status.ready||resourceInstallationBusy(resource.id))continue;try{if(status.download.state==='downloaded')await requestResourceAction(resource.id,'prepare/start');else await requestResourceAction(resource.id,'download/start')}catch(error){failures.push(`${resourceTitle(resource.id)}: ${error.message}`)}}if(resources.some(item=>item.installMode==='stream')){try{const response=await fetch(`/api/profiles/${profileId}/prepare/start?concurrency=${installationConcurrency()}&sourceMode=${sourceInputMode()}`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();if(!response.ok)throw new Error(body.error||'Profile installation could not start')}catch(error){failures.push(error.message)}}if(!failures.length)dialog?.close()}catch(error){failures.push(error.message)}finally{if(button)button.disabled=false}await refreshAppStatus();if(failures.length)showResourceNotice(`Some installations could not start: ${failures.join(' · ')}`)}
favorOnline=createFavorOnline({
  $,escapeHtml,prototypeIcon,openFluentDialog,
  togglePopover:()=>toggleResultPopover('favor-popover'),
  getState:()=>({currentResultRun,resultTotal,selectionCount:selectionCount()}),
  collectFilteredAlleles:()=>filteredAlleleIds(new Set(),resultTotal),
  collectSelectedAlleles:()=>selectionMode==='filtered'?filteredSelectionAlleleIds():Promise.resolve([...selectedAlleles]),
  refreshResultSchema:refreshCurrentResultSchema,
  setResultStatus:setFavorResultStatus,
  showNotice:message=>{const heading=document.querySelector('#results .results-heading p');if($('#results').classList.contains('active-page')&&heading)heading.textContent=message;else showResourceNotice(message)},
  onServiceChange:()=>updateWizardReadiness()
});
async function start(){
  [variants,sources,profiles,resourcePlan,evidenceCalibrations,portablePaths]=await Promise.all([fetch('/api/demo/variants').then(r=>r.json()),fetch('/api/sources').then(r=>r.json()),fetch('/api/profiles').then(r=>r.json()),fetch('/api/resources/plan').then(r=>r.json()),fetch('/api/evidence-calibrations').then(r=>r.json()),fetch('/api/paths').then(r=>r.json())]);
  if(portablePaths.runs){$('#output-folder').value=portablePaths.runs;$('#folder-message').textContent='Default results directory. Use Browse to change this run only.'}
  $('#settings-resource-path').value=portablePaths.resourceDirectory||'Unavailable';
  $('#settings-downloads-path').value=portablePaths.downloads||'Unavailable';
  $('#settings-results-path').value=portablePaths.runs||'Unavailable';
  renderColumns();renderTable();renderProfiles();renderWizardSources();normalizeSourceCatalogControls();bindSourceActionMenus();
  await favorOnline.initialize().catch(error=>showResourceNotice(error.message));
  document.addEventListener('click',event=>{const install=event.target.closest('[data-install]'),core=event.target.closest('[data-core-install]'),jobButton=event.target.closest('[data-job-action]'),jobCard=jobButton?.closest('[data-download-job]'),annotationCard=jobButton?.closest('[data-annotation-task]');if(install)handleResourceCardInstall(install);if(core)startCoreInstall();if(jobButton&&jobCard)handleDownloadJobAction(jobCard.dataset.downloadJob,jobButton.dataset.jobAction,jobButton);if(jobButton&&annotationCard)handleAnnotationTaskAction(annotationCard.dataset.annotationTask,jobButton.dataset.jobAction,jobButton);const page=event.target.closest('[data-page-link]')?.dataset.pageLink;if(page){setupDismissed=true;showPage(page)}});
  document.querySelectorAll('[data-pick-storage]').forEach(button=>button.addEventListener('click',chooseSettingsFolder));
  const savedProfile=localStorage.getItem('annocat.defaultProfile')||'wgs',showSetup=localStorage.getItem('annocat.showSetup')!=='false';
  localStorage.removeItem('annocat.resultDensity');
  document.body.classList.remove('compact-results');
  $('#default-profile').value=savedProfile;$('#show-setup').checked=showSetup;
  const profileOption=[...$('#profile').options].find(option=>option.value===savedProfile);if(profileOption){$('#profile').value=savedProfile;renderWizardSources()}
  setupDismissed=!showSetup;
  $('#show-setup').addEventListener('change',event=>{localStorage.setItem('annocat.showSetup',event.target.checked);setupDismissed=!event.target.checked;if(setupDismissed)$('#first-run').classList.remove('visible');else updateSetupModal(lastSetupReady)});
  $('#default-profile').addEventListener('change',event=>localStorage.setItem('annocat.defaultProfile',event.target.value));
  $('#reset-preferences').addEventListener('click',()=>{localStorage.removeItem('annocat.showSetup');localStorage.removeItem('annocat.defaultProfile');localStorage.removeItem('annocat.installationConcurrency');localStorage.removeItem('annocat.sourceInputMode');favorOnline.resetConfirmation();$('#show-setup').checked=true;$('#default-profile').value='wgs';installationConcurrencySelect.value='1';sourceInputModeSelect.value='resumable'});
  await Promise.all([refreshAppStatus(),refreshCompletedRuns()]);
  setInterval(()=>refreshAppStatus().catch(console.error),1000);setInterval(()=>{if($('#browse').classList.contains('active-page'))refreshCompletedRuns().catch(console.error)},5000)
}
document.querySelectorAll('.nav-item').forEach(button=>button.addEventListener('click',()=>{showPage(button.dataset.page);if(button.dataset.page==='browse')refreshCompletedRuns().catch(console.error)}));$('#open-demo').addEventListener('click',async()=>{variants=await fetch('/api/demo/variants').then(response=>response.json());renderTable();document.querySelector('#results .results-heading h1').textContent='Synthetic demonstration';document.querySelector('#results .results-heading p').textContent='No personal variant files are loaded.';showPage('results')});$('#back-to-browse').addEventListener('click',()=>{showPage('browse');refreshCompletedRuns().catch(console.error)});$('#choose-vcfs').addEventListener('click',()=>chooseVcfs().catch(error=>showResourceNotice(error.message)));$('#recover-annotation').addEventListener('click',()=>chooseRecoveryFiles().catch(error=>showResourceNotice(error.message)));$('#vcf-files').addEventListener('change',event=>{recoveryFiles=null;selectedPaths=[...event.target.files].map(file=>file.name);renderSelectedPaths()});$('#profile').addEventListener('change',applyProfile);$('#browse-output').addEventListener('click',chooseFolder);$('#output-directory-fallback').addEventListener('change',event=>{const file=event.target.files[0];if(file){const folder=file.webkitRelativePath.split('/')[0];$('#output-folder').value=folder;$('#folder-message').textContent=`Selected “${folder}” using compatibility mode.`;setStep(3)}});$('#output-folder').addEventListener('input',()=>setStep(3));$('#continue').addEventListener('click',()=>{if(currentStep<4)setStep(currentStep+1);else startAnnotation()});$('#back-step').addEventListener('click',()=>setStep(currentStep-1));$('#search').addEventListener('input',scheduleResultSearch);$('#columns').addEventListener('click',event=>{event.stopPropagation();toggleResultPopover('column-menu')});start().catch(error=>console.error(error));
document.addEventListener('click',event=>{const pageButton=event.target.closest('[data-status-page]'),disableButton=event.target.closest('[data-status-disable-source]'),dismissButton=event.target.closest('[data-status-dismiss]');if(pageButton)showPage(pageButton.dataset.statusPage);if(disableButton){const sourceId=disableButton.dataset.statusDisableSource,input=document.querySelector(`#wizard-sources input[data-source="${sourceId}"]`);if(input){input.checked=false;$('#profile').value='custom';$('#wizard-sources .profile-badge').forEach(badge=>badge.remove())}clearGlobalStatusNotice();showPage('annotate');setStep(4);refreshAppStatus().catch(console.error)}if(dismissButton)clearGlobalStatusNotice()});
$('#setup-annotation').addEventListener('click',()=>{setupDismissed=true;$('#first-run').classList.add('hidden');showPage('resources')});$('#setup-open-results').addEventListener('click',openExistingResults);$('#setup-later').addEventListener('click',()=>{setupDismissed=true;$('#first-run').classList.add('hidden')});document.querySelector('#browse .choice:first-child').addEventListener('click',openExistingResults);
document.addEventListener('click',event=>{const button=event.target.closest('[data-update]');if(button)checkSourceUpdate(button.dataset.update,button)});
const installationConcurrencySelect=$('#installation-concurrency');
installationConcurrencySelect.value=String(installationConcurrency());
installationConcurrencySelect.addEventListener('change',event=>setInstallationConcurrency(event.target.value));
const sourceInputModeSelect=$('#source-input-mode');
sourceInputModeSelect.value=sourceInputMode();
sourceInputModeSelect.addEventListener('change',event=>setSourceInputMode(event.target.value));
document.addEventListener('click',event=>{if(event.target.closest('[data-dbnsfp-config]'))showDbnsfpFieldConfiguration()});
document.addEventListener('click',event=>{const button=event.target.closest('[data-source-fields-config]');if(button)showSupplementaryFieldConfiguration(button.dataset.sourceFieldsConfig)});
document.addEventListener('click',event=>{const button=event.target.closest('[data-profile-install-fields]');if(button)editProfileInstallFields(button)});
renderFilterRules();
refreshFilterPresetSelector();
const resultTableWrap=$('.table-wrap'),resultScrollSentinel=$('#result-scroll-sentinel');
const resultScrollObserver=new IntersectionObserver(entries=>{if(entries.some(entry=>entry.isIntersecting))loadMoreResults()},{root:resultTableWrap,rootMargin:'500px 0px'});resultScrollObserver.observe(resultScrollSentinel);
resultTableWrap.addEventListener('scroll',()=>{if(resultTableWrap.scrollHeight-resultTableWrap.scrollTop-resultTableWrap.clientHeight<500)loadMoreResults()},{passive:true});
$('#open-demo').addEventListener('click',()=>{currentResultRun=null},true);
$('#rows').addEventListener('click',event=>{const rowElement=event.target.closest('[data-allele-id]');if(!rowElement)return;const index=variants.findIndex(item=>item.alleleId===rowElement.dataset.alleleId),row=variants[index],checkbox=event.target.closest('[data-select-allele]'),candidateButton=event.target.closest('[data-toggle-candidate]');if(index<0)return;if(candidateButton){event.preventDefault();event.stopPropagation();candidateButton.disabled=true;setCandidateMembership([row.alleleId],!candidateAlleles.has(row.alleleId)).catch(error=>{document.querySelector('#results .results-heading p').textContent=`Could not update candidates: ${error.message}`});return}if(event.shiftKey){event.preventDefault();selectVariantRange(index,{additive:event.ctrlKey||event.metaKey});return}if(checkbox){if(selectionMode==='filtered'){checkbox.checked?excludedFilteredAlleles.delete(row.alleleId):excludedFilteredAlleles.add(row.alleleId)}else setVariantSelected(row,checkbox.checked);selectionAnchorIndex=index;renderTable();return}if(event.ctrlKey||event.metaKey){event.preventDefault();if(selectionMode==='filtered'){excludedFilteredAlleles.has(row.alleleId)?excludedFilteredAlleles.delete(row.alleleId):excludedFilteredAlleles.add(row.alleleId)}else setVariantSelected(row,!selectedAlleles.has(row.alleleId));selectionAnchorIndex=index;renderTable();return}rowElement.focus({preventScroll:true});selectionAnchorIndex=index;openVariantDetail(row.alleleId)});
function moveVariantDetailSelection(step){
  const rows=[...$('#rows').querySelectorAll('tr[data-allele-id]')];if(!rows.length)return;
  const focused=document.activeElement?.closest?.('tr[data-allele-id]'),currentIndex=rows.findIndex(row=>row.dataset.alleleId===(focused?.dataset.alleleId||selectedAlleleId)),nextIndex=currentIndex<0?(step>0?0:rows.length-1):Math.max(0,Math.min(rows.length-1,currentIndex+step)),next=rows[nextIndex];
  if(!next||nextIndex===currentIndex)return;
  selectionAnchorIndex=variants.findIndex(row=>row.alleleId===next.dataset.alleleId);next.focus({preventScroll:true});next.scrollIntoView({block:'nearest',inline:'nearest'});openVariantDetail(next.dataset.alleleId)
}
$('#rows').addEventListener('keydown',event=>{if(event.target.matches('input,button,a,select,textarea'))return;if(['ArrowUp','ArrowDown'].includes(event.key)){event.preventDefault();moveVariantDetailSelection(event.key==='ArrowDown'?1:-1);return}if(['Enter',' '].includes(event.key)){event.preventDefault();openVariantDetail(event.target.closest('[data-allele-id]')?.dataset.alleleId)}});
$('#close-variant-detail').addEventListener('click',closeVariantDetail);
$('#detail-candidate-toggle').addEventListener('click',async event=>{const button=event.currentTarget,alleleId=button.dataset.candidateAllele;if(!alleleId||button.disabled)return;button.disabled=true;const add=!candidateAlleles.has(alleleId);try{await setCandidateMembership([alleleId],add);if(!add&&resultView==='candidates'){closeVariantDetail();return}renderCandidateDetailControl(alleleId)}catch(error){showGlobalStatus(error.message,'error')}finally{button.disabled=false}});
$('#share-report').addEventListener('click',shareCurrentReport);
const resultPopovers=[['result-filters','filters'],['column-menu','columns'],['favor-popover','favor']];
function closeResultPopovers(){resultPopovers.forEach(([panel,button])=>{$(`#${panel}`).classList.add('hidden');$(`#${button}`).setAttribute('aria-expanded','false')})}
function toggleResultPopover(panelId){
  const entry=resultPopovers.find(([panel])=>panel===panelId),panel=$(`#${panelId}`),open=panel.classList.contains('hidden');
  closeResultPopovers();
  if(!open||!entry)return;
  const button=$(`#${entry[1]}`),container=$('#results .results-panel'),containerBounds=container.getBoundingClientRect(),buttonBounds=button.getBoundingClientRect();
  panel.style.top=`${Math.round(buttonBounds.bottom-containerBounds.top-container.clientTop+6)}px`;
  panel.classList.remove('hidden');
  if(panel.classList.contains('fui-popover--anchor-start')||panel.classList.contains('fui-popover--anchor-center')){
    const buttonLeft=buttonBounds.left-containerBounds.left-container.clientLeft;
    const preferredLeft=panel.classList.contains('fui-popover--anchor-center')
      ? buttonLeft+(buttonBounds.width-panel.offsetWidth)/2
      : buttonLeft;
    const maximumLeft=Math.max(14,container.clientWidth-panel.offsetWidth-14);
    panel.style.right='auto';
    panel.style.left=`${Math.round(Math.max(14,Math.min(preferredLeft,maximumLeft)))}px`
  }else{
    panel.style.removeProperty('left');
    panel.style.removeProperty('right')
  }
  button.setAttribute('aria-expanded','true');
  $('#selection-actions-menu').classList.add('hidden');
  $('#selection-actions-toggle').setAttribute('aria-expanded','false');
  if(panelId==='column-menu')requestAnimationFrame(()=>panel.querySelector('[data-column-search]')?.focus({preventScroll:true}))
}
$('#column-menu').addEventListener('keydown',event=>{
  if(event.key!=='Escape')return;
  event.preventDefault();
  closeResultPopovers();
  $('#columns').focus()
});
$('#filters').addEventListener('click',event=>{event.stopPropagation();toggleResultPopover('result-filters')});
$('#phenotypes').addEventListener('click',openPhenotypeDialog);
$('#result-filters').addEventListener('input',event=>{if(!event.target.matches('[data-filter-column-search]'))filterRulesChanged()});
$('#result-filters').addEventListener('change',event=>{if(!event.target.closest('.filter-preset-bar'))filterRulesChanged()});
$('#apply-filters').addEventListener('click',()=>{const error=validateResultFilters();$('#filter-message').textContent=error;if(!error&&currentResultRun){resultPageMemory.clear();resultOperation='Filtering…';resultLoading=true;updateResultPageStatus();openCompletedRun(currentResultRun,0)}});
$('#reset-filters').addEventListener('click',()=>clearResultFilters(true));
$('#add-filter-rule').addEventListener('click',()=>{addFilterRule({column:'gene',operator:'in',value:''});filterRulesChanged()});
$('#save-filter-preset').addEventListener('click',saveFilterPreset);
$('#load-filter-preset').addEventListener('click',loadFilterPreset);
$('#delete-filter-preset').addEventListener('click',deleteFilterPreset);
$('#result-filters').addEventListener('keydown',event=>{if(event.key==='Enter'&&!event.target.closest('.filter-preset-bar')){event.preventDefault();const error=validateResultFilters();$('#filter-message').textContent=error;if(!error&&currentResultRun)openCompletedRun(currentResultRun,0)}});
$('#candidate-selected').addEventListener('click',updateSelectedCandidates);
$('#all-variants-tab').addEventListener('click',()=>changeResultView('all'));
$('#candidates-tab').addEventListener('click',()=>changeResultView('candidates'));
$('#export-selected-genes').addEventListener('click',exportSelectedGenes);
$('#export-selected-rows').addEventListener('click',exportSelectedRows);
$('#selection-actions-toggle').addEventListener('click',event=>{event.stopPropagation();closeResultPopovers();const menu=$('#selection-actions-menu'),open=menu.classList.toggle('hidden')===false;event.currentTarget.setAttribute('aria-expanded',String(open))});
$('#selection-actions-menu').addEventListener('click',()=>{$('#selection-actions-menu').classList.add('hidden');$('#selection-actions-toggle').setAttribute('aria-expanded','false')});
document.addEventListener('click',event=>{if(!event.target.closest('#selection-actions')){$('#selection-actions-menu').classList.add('hidden');$('#selection-actions-toggle').setAttribute('aria-expanded','false')}resultPopovers.forEach(([panel,button])=>{if(!event.target.closest(`#${panel}`)&&!event.target.closest(`#${button}`)){$(`#${panel}`).classList.add('hidden');$(`#${button}`).setAttribute('aria-expanded','false')}});document.querySelectorAll('.filter-column-picker').forEach(picker=>{if(!picker.contains(event.target))closeFilterColumnPicker(picker)})});
$('#rename-report').addEventListener('click',renameCurrentReport);
$('#case-notes-button').addEventListener('click',toggleCaseNotes);
$('#save-case-notes').addEventListener('click',saveCaseNotes);
$('#revert-case-notes').addEventListener('click',()=>{$('#case-notes-editor').value=loadedCaseNotes;$('#case-notes-status').textContent='Reverted to last save'});
$('#case-notes-editor').addEventListener('input',()=>{clearTimeout(caseNotesTimer);$('#case-notes-status').textContent='Unsaved changes';caseNotesTimer=setTimeout(()=>saveCaseNotes().catch(error=>{$('#case-notes-status').textContent=error.message}),800)});

const variantPresentation=createVariantPresentation({
  $,columns,escapeHtml,prototypeIcon,resultFieldIdentity,fieldSourceIs,resourceTitle,
  coreColumnPresentation,displayColumns,moveResultColumn,decodeEvidenceValue,resultColumnRawValue,
  renderTableBase,revealVariantDetail,consequenceValue,usefulVariantLinks,displayDetailValue,
  dbnsfpPredictionValue,evidenceFieldPresentationBase,
  getState:()=>({resultFieldCatalog,evidenceCalibrations,variants,resultAlignmentGroups})
});
function evidenceFieldPresentation(...args){return variantPresentation.evidenceFieldPresentation(...args)}
function consequenceTerms(...args){return variantPresentation.consequenceTerms(...args)}
function evidenceValuePresentation(...args){return variantPresentation.evidenceValuePresentation(...args)}
function renderVariantDetail(...args){return variantPresentation.renderVariantDetail(...args)}
function renderTable(...args){return variantPresentation.renderTable(...args)}
