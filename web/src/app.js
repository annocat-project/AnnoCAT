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
const resultQuerySession=typeof globalThis.crypto?.randomUUID==='function'?globalThis.crypto.randomUUID():`${Date.now()}-${Math.random()}`;
const RESULT_PAGE_MEMORY_LIMIT=12,resultPageMemory=new Map(),RESULT_VIEW_MEMORY_LIMIT=4,resultViewMemory=new Map(),VARIANT_DETAIL_MEMORY_LIMIT=64,variantDetailMemory=new Map();
const pageNames={annotate:'New annotation',browse:'Browse results',results:'Results',logs:'Tasks',resources:'Data sources',settings:'Settings'};
let variants=[],sources=[],profiles=[],resourcePlan={resources:[]},evidenceCalibrations={interpretationPolicy:{},predictors:[],calibrations:[],displayPolicies:{}},portablePaths={},visible=new Set(columns.filter(([, ,shown])=>shown).map(([key])=>key)),visibleEvidence=new Set(),resultColumnOrder=[],currentStep=1,selectedPaths=[],selectedVcfSummaries=[],recoveryFiles=null;
let humanReadableColumnNames=localStorage.getItem('annocat.humanReadableColumnNames')!=='false',resultSorts=[];
let setupDismissed=false,lastTaskSnapshots=[],lastAnnotationState={state:'idle'},globalStatusNotice=null,completedRuns=[],lastSetupReady=false,resourceStates={},refreshingResources=false,currentResultRun=null,resultView='all',candidateAlleles=new Set(),resultOffset=0,resultTotal=0,resultCountSignature='',resultNaturalOrderSignature='',resultNaturalOrder=new Map(),selectedAlleleId=null,resultFieldCatalog=[],resultAlignmentGroups=[],resultLoading=false,resultHasMore=false,resultRequestGeneration=0,resultQuerySignature='',resultRequestController=null,loadedCaseNotes='',caseNotesTimer=null,caseNotesRunId=null,selectionRunId=null,selectionAnchorIndex=null,selectedAlleles=new Set(),excludedFilteredAlleles=new Set(),selectedVariantGenes=new Map(),selectedVariantRows=new Map(),selectionMode='explicit',selectionFilterSignature='',dbnsfpConfiguration=null,supplementaryFieldConfigurations=new Map();
let resultSearchTimer=null,resultOperation='',resultQueryError='';
let phenotypeProfile=null,phenotypeExploration=null,phenotypeDialogRunId=null,phenotypeSampleName='',phenotypeSearchTimer=null,phenotypeResultLimit=100,phenotypeSearchResults=[],phenotypeSearchActiveIndex=-1,phenotypeResultSort='phenotype',phenotypeMessage='',phenotypeOnlineConsent=false,phenotypeSaveRevision=0,phenotypeSaveChain=Promise.resolve();
const phenotypeSampleSelections=new Map();
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
function recommendedEvidenceIndexes(){const entries=selectableEvidenceEntries(),selected=[];const pick=(...tests)=>{for(const test of tests){const match=entries.find(({field})=>test(field));if(match&&!selected.includes(match.index)){selected.push(match.index);return}}};const leaf=field=>String(field.fieldPath||'').split(/[.\[\]]/).filter(Boolean).pop()?.toLowerCase()||'';pick(field=>fieldSourceIs(field,'clinvar')&&field.scope==='allele'&&leaf(field)==='significance');pick(field=>String(field.sourceId||'').toLowerCase().includes('gnomad')&&['allaf','af','allele_frequency'].includes(leaf(field)));pick(field=>fieldSourceIs(field,'cadd')&&leaf(field)==='phred',field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='cadd_phred');pick(field=>fieldSourceIs(field,'revel')&&leaf(field)==='score',field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='revel_score');pick(field=>fieldSourceIs(field,'dbnsfp')&&leaf(field)==='alphamissense_score');pick(field=>fieldSourceIs(field,'phylop')&&leaf(field)==='score',field=>fieldSourceIs(field,'dbnsfp')&&leaf(field).includes('phylop'));return selected.slice(0,32)}
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
  const layout=$('#results .results-layout'),detail=$('#variant-detail'),tableWrap=$('#results .table-wrap'),toolbar=$('#results .toolbar'),tabs=$('#results .result-view-tabs');if(!layout||!detail)return;
  if(tableWrap&&toolbar&&tabs&&!detail.dataset.headingAlignment){const alignHeading=()=>{const tableBounds=tableWrap.getBoundingClientRect(),borderTop=parseFloat(getComputedStyle(tableWrap).borderTopWidth)||0,height=Math.max(45,tableBounds.top+borderTop-layout.getBoundingClientRect().top);detail.style.setProperty('--annocat-detail-heading-height',`${height}px`)};const observer=new ResizeObserver(()=>requestAnimationFrame(alignHeading));[layout,tableWrap,toolbar,tabs].forEach(element=>observer.observe(element));detail.dataset.headingAlignment='true';alignHeading()}
  if(detail.querySelector('.variant-detail-resizer'))return;
  const handle=document.createElement('span');handle.className='variant-detail-resizer';handle.setAttribute('role','separator');handle.setAttribute('aria-label','Resize variant details');handle.setAttribute('aria-orientation','vertical');handle.tabIndex=0;handle.title='Drag to resize variant details; double-click to reset';detail.prepend(handle);
  const applyWidth=width=>{const bounds=layout.getBoundingClientRect(),maximum=Math.max(320,Math.min(720,bounds.width*.55,bounds.width-420)),value=Math.round(Math.max(320,Math.min(maximum,width)));layout.style.setProperty('--annocat-detail-width',`${value}px`);handle.setAttribute('aria-valuenow',String(value));sessionStorage.setItem('annocat.detailWidth',String(value))};
  const stored=Number(sessionStorage.getItem('annocat.detailWidth'));if(Number.isFinite(stored)&&stored>0)applyWidth(stored);
  handle.addEventListener('pointerdown',event=>{event.preventDefault();const move=moveEvent=>applyWidth(layout.getBoundingClientRect().right-moveEvent.clientX),up=()=>{window.removeEventListener('pointermove',move);window.removeEventListener('pointerup',up)};window.addEventListener('pointermove',move);window.addEventListener('pointerup',up)});
  handle.addEventListener('keydown',event=>{if(!['ArrowLeft','ArrowRight'].includes(event.key))return;event.preventDefault();const current=detail.getBoundingClientRect().width;applyWidth(current+(event.key==='ArrowLeft'?20:-20))});
  handle.addEventListener('dblclick',()=>{layout.style.removeProperty('--annocat-detail-width');sessionStorage.removeItem('annocat.detailWidth')});
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
function openAbout(){aboutReturnFocus=document.activeElement;const overlay=$('#about-overlay');overlay.classList.remove('hidden');overlay.classList.add('visible');document.body.classList.add('modal-open');$('#about-dialog').focus();loadAboutMetadata()}
function closeAbout(){const overlay=$('#about-overlay');overlay.classList.add('hidden');overlay.classList.remove('visible');document.body.classList.remove('modal-open');if(aboutReturnFocus?.focus)aboutReturnFocus.focus()}
$('#about-button').addEventListener('click',openAbout);
$('#about-close').addEventListener('click',closeAbout);
$('#about-overlay').addEventListener('click',event=>{if(event.target===event.currentTarget)closeAbout()});
document.addEventListener('keydown',event=>{if(event.key==='Escape'&&!$('#about-overlay').classList.contains('hidden'))closeAbout()});
function updateSetupModal(){const hasInstalledResources=Object.values(resourceStates).some(state=>state?.ready),hidden=lastSetupReady||hasInstalledResources||setupDismissed;$('#first-run').classList.toggle('hidden',hidden);$('#first-run').classList.toggle('visible',!hidden)}
function resourceTitle(id){return id==='grch38-reference'?'GRCh38 reference':id==='ensembl-gff3'?'Ensembl transcript cache':sources.find(source=>source.id===id)?.name||id}
function taskActivityLabel(task){if(task.state!=='running')return{queued:'Queued',validating:'Verifying',cancelling:'Stopping and discarding',interrupted:'Interrupted · Resume available',failed:'Needs attention',paused:'Paused',cancelled:'Paused',downloaded:'Ready to install',ready:'Installed',completed:'Completed'}[task.state]||task.phase||task.state;return{'recovery-scan':'Scanning interrupted output','recovery-input':'Preparing remaining input','recovery-merge':'Joining recovered output','indexing-variants':'Building variant table','indexing-evidence':'Building evidence tables','reconnecting':'Reconnecting','retrying':'Reconnecting','replaying':'Replaying','building-cache':'Building cache','downloading-source-part':'Downloading','downloading':'Downloading','streaming-to-fastvep':'Streaming','validating':'Verifying','reading-index':'Reading index','reading-indexes':'Reading index','publishing':'Publishing'}[task.phase]||(task.kind==='installation'?'Installing':task.kind==='download'?'Downloading':'Annotating')}
function taskJobView(task){const active=['queued','running','validating','cancelling'].includes(task.state),kind=active?'active':['failed','interrupted'].includes(task.state)?'failed':['ready','completed'].includes(task.state)?'completed':'pending';return{state:taskActivityLabel(task),kind,detail:task.error||task.detail||'',name:task.title||'Task',cancellable:task.kind==='annotation'&&(task.availableActions||[]).includes('cancel'),percent:Number.isFinite(Number(task.percent))?Number(task.percent):null}}
function taskActionButtons(task,actions=task.availableActions||[]){const labels={pause:'Pause',resume:'Resume',install:'Install',cancel:'Stop & discard',remove:'Remove data'};return actions.map(action=>`<button type="button" class="${action==='cancel'?'cancel':action}" data-job-action="${escapeHtml(action)}">${labels[action]||escapeHtml(action)}</button>`).join('')}
function confirmDestructiveAction({title,message,confirmLabel,cancelLabel='Keep data'}){
  let dialog=$('#confirm-action-dialog');
  if(!dialog){
    document.body.insertAdjacentHTML('beforeend','<dialog id="confirm-action-dialog" class="install-review confirmation-dialog" aria-labelledby="confirm-action-title"><form method="dialog"><p class="kicker">Confirm action</p><h2 id="confirm-action-title"></h2><p data-confirm-action-message></p><div class="install-review-actions"><button type="submit" value="cancel" data-confirm-action-cancel>Cancel</button><button type="submit" value="confirm" class="danger-button" data-confirm-action-confirm>Confirm</button></div></form></dialog>');
    dialog=$('#confirm-action-dialog')
  }
  dialog.querySelector('#confirm-action-title').textContent=title;
  dialog.querySelector('[data-confirm-action-message]').textContent=message;
  dialog.querySelector('[data-confirm-action-cancel]').textContent=cancelLabel;
  dialog.querySelector('[data-confirm-action-confirm]').textContent=confirmLabel;
  dialog.returnValue='';
  return new Promise(resolve=>{
    dialog.addEventListener('close',()=>resolve(dialog.returnValue==='confirm'),{once:true});
    dialog.showModal()
  })
}
function formatEta(seconds){const value=Math.max(0,Number(seconds)||0);if(value<60)return`about ${Math.ceil(value)} seconds remaining`;if(value<3600)return`about ${Math.ceil(value/60)} minutes remaining`;return`about ${(value/3600).toFixed(value>=36000?0:1)} hours remaining`}
function annotationTaskMeta(task){if(String(task.phase||'').startsWith('indexing-')||task.phase==='publishing'){const parts=[];if(task.detail)parts.push(task.detail);const completed=Number(task.completedRecords||0),total=Number(task.totalRecords||0),bytes=Number(task.completedBytes||0),recordSpeed=Number(task.throughputRecordsPerSecond||0),byteSpeed=Number(task.throughputBytesPerSecond||0);if(total>0)parts.push(`${completed.toLocaleString()} of ${total.toLocaleString()} variants`);if(bytes>0)parts.push(`${formatDataSize(bytes)} written`);if(recordSpeed>0)parts.push(`${Math.round(recordSpeed).toLocaleString()} variants/s`);else if(byteSpeed>0)parts.push(`${formatDataSize(byteSpeed)}/s`);if(task.etaSeconds!==null&&task.etaSeconds!==undefined&&Number(task.etaSeconds)>0)parts.push(formatEta(task.etaSeconds));return parts.join(' · ')}const parts=[];if(task.chromosome)parts.push(`Chromosome ${task.chromosome}`);const completed=Number(task.completedRecords||0),total=Number(task.totalRecords||0);if(total>0)parts.push(`${completed.toLocaleString()} of ${total.toLocaleString()} variants`);const recordSpeed=Number(task.throughputRecordsPerSecond||0),byteSpeed=Number(task.throughputBytesPerSecond||0);if(recordSpeed>0)parts.push(`${Math.round(recordSpeed).toLocaleString()} variants/s`);if(byteSpeed>0)parts.push(`${formatDataSize(byteSpeed)}/s`);if(task.etaSeconds!==null&&task.etaSeconds!==undefined&&Number(task.etaSeconds)>0)parts.push(formatEta(task.etaSeconds));return parts.join(' · ')}
function providerErrorSource(message){return String(message||'').match(/^([a-z0-9-]+) is selected but /i)?.[1]||null}
function renderAnnotationNotice(){const notice=$('#annotation-notice');if(!notice)return;if(globalStatusNotice?.kind!=='annotation'){notice.classList.add('hidden');notice.innerHTML='';return}const sourceId=providerErrorSource(globalStatusNotice.message);notice.innerHTML=`<strong>Annotation could not start</strong><p>${escapeHtml(globalStatusNotice.message)}</p><div class="global-status-actions">${sourceId?`<button type="button" data-status-disable-source="${escapeHtml(sourceId)}">Continue without ${escapeHtml(resourceTitle(sourceId))}</button>`:''}<button type="button" data-status-page="resources">Manage data sources</button><button type="button" data-status-dismiss>Dismiss</button></div>`;notice.classList.remove('hidden')}
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
function renderJobs(){
  const jobs=lastTaskSnapshots.map(task=>({task,view:taskJobView(task)})),active=jobs.filter(({view})=>view.kind==='active').length,completed=jobs.filter(({view})=>view.kind==='completed').length,failed=jobs.filter(({view})=>view.kind==='failed').length;
  $('#active-job-count').textContent=active;
  $('#completed-job-count').textContent=completed;
  $('#failed-job-count').textContent=failed;
  const cards=jobs.map(({task,view})=>{
    if(task.resourceId&&view.kind!=='completed')return resourceTaskHtml(task);
    const annotation=task.kind==='annotation',meta=annotation?annotationTaskMeta(task):'',percent=Math.max(0,Math.min(100,Number(task.percent)||0)),actions=annotation?taskActionButtons(task):task.resourceId?taskActionButtons(task):'';
    return`<article class="download-job log-job-card" ${task.resourceId?`data-download-job="${escapeHtml(task.resourceId)}"`:''}${annotation?` data-annotation-task="${escapeHtml(task.runId||'')}"`:''}><div class="download-job-head"><div><strong>${escapeHtml(view.name)}</strong><small>${escapeHtml(view.state)}</small></div><div class="download-job-actions"><time>${escapeHtml(task.updatedAt?formatDateTime(task.updatedAt):view.kind==='completed'?'Installed':'Current')}</time>${actions}</div></div>${annotation&&percent>0&&view.kind!=='completed'?`<div class="download-progress-meta"><span>${escapeHtml(meta||view.detail)}</span><strong>${percent.toFixed(1)}%</strong></div><div class="progress-track"><div class="progress-fill" style="width:${percent}%"></div></div>`:`<div class="download-detail"><span>${escapeHtml(meta||view.detail)}</span></div>`}</article>`
  }).filter(Boolean);
  $('#jobs-list').innerHTML=cards.length?cards.join(''):'<div class="empty-card compact"><span>✓</span><h2>No tasks yet</h2><p>Downloads, installations, and annotations will appear here.</p></div>';
  renderGlobalStatus()
}
function renderCompletedRuns(runs){const host=$('#completed-runs');host.innerHTML=runs.length?runs.map(run=>{const size=formatResultBytes(run.canonicalResultBytes);return`<button type="button" class="completed-run" data-completed-run="${escapeHtml(run.id)}"><span><strong>${escapeHtml(run.name)}</strong><small>${escapeHtml(formatDateTime(run.completedAt))} · ${escapeHtml(run.assembly)} · ${Number(run.variantCount).toLocaleString()} variants${size?` · ${size} canonical results`:''}</small></span><b>Open →</b></button>`}).join(''):'<div class="empty-card compact"><span>□</span><h2>No completed annotations yet</h2><p>Finished annotations will appear here automatically.</p></div>';host.querySelectorAll('[data-completed-run]').forEach(button=>button.addEventListener('click',()=>openCompletedRun(runs.find(item=>item.id===button.dataset.completedRun))))}
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
    $('#search').value='';
    caseNotesRunId=null;
    $('#case-notes-panel').classList.add('hidden');
    clearResultFilters(false);
    currentResultRun=run;
    resultView='all';
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
function updateResultScrollState(){const sentinel=$('#result-scroll-sentinel');if(!sentinel)return;sentinel.textContent=resultLoading?(variants.length?'Loading more variants…':''):resultHasMore?'Scroll to load more':variants.length?'All available variants loaded':'';sentinel.classList.toggle('hidden',!sentinel.textContent)}
function loadMoreResults(){if(!currentResultRun||resultLoading||!resultHasMore)return;openCompletedRun(currentResultRun,variants.length)}
async function refreshCompletedRuns(){const response=await fetch('/api/runs'),body=await response.json();if(!response.ok)throw new Error(body.error||'Completed annotations unavailable');completedRuns=body.runs||[];renderCompletedRuns(completedRuns);renderJobs()}
async function openExistingResults(){const response=await fetch('/api/pick-results',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok){document.querySelector('#browse .page-heading>p:last-child').textContent=result.error||'Could not open results';showPage('browse');return}if(result.runId){setupDismissed=true;$('#first-run').classList.add('hidden');await refreshCompletedRuns();const run=completedRuns.find(item=>item.id===result.runId);if(run)await openCompletedRun(run)}}
async function shareCurrentReport(){if(!currentResultRun)return;const button=$('#share-report'),description=document.querySelector('#results .results-heading p'),previous=button.textContent;button.disabled=true;button.textContent='Creating…';try{const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/share`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();if(!response.ok)throw new Error(body.error||'Report could not be created');if(body.path)description.textContent=`Shared report created · ${formatResultBytes(body.bytes)} · ${body.path}`;}catch(error){description.textContent=`Could not share this report: ${error.message}`}finally{button.disabled=false;button.textContent=previous}}
async function renameCurrentReport(){if(!currentResultRun)return;const name=prompt('Report name',currentResultRun.name);if(name===null||name.trim()===currentResultRun.name)return;const response=await fetch(`/api/runs/${encodeURIComponent(currentResultRun.id)}/name`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({name:name.trim()})}),body=await response.json();if(!response.ok){document.querySelector('#results .results-heading p').textContent=`Could not rename this report: ${body.error||'Unknown error'}`;return}currentResultRun.name=body.name;document.querySelector('#results .results-heading h1').textContent=body.name;await refreshCompletedRuns()}
async function loadCaseNotes(){if(!currentResultRun)return;const runId=currentResultRun.id;caseNotesRunId=runId;$('#case-notes-status').textContent='Loading…';const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/notes`),body=await response.json();if(caseNotesRunId!==runId)return;if(!response.ok){$('#case-notes-status').textContent=body.error||'Could not load notes';return}loadedCaseNotes=body.notes||'';$('#case-notes-editor').value=loadedCaseNotes;$('#case-notes-status').textContent='Saved locally'}
async function saveCaseNotes(){const runId=caseNotesRunId;if(!runId)return;clearTimeout(caseNotesTimer);const notes=$('#case-notes-editor').value;$('#case-notes-status').textContent='Saving…';const response=await fetch(`/api/runs/${encodeURIComponent(runId)}/notes`,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({notes})}),body=await response.json();if(caseNotesRunId!==runId)return;if(!response.ok){$('#case-notes-status').textContent=body.error||'Could not save notes';return}loadedCaseNotes=notes;$('#case-notes-status').textContent='Saved locally'}
async function toggleCaseNotes(){if(!currentResultRun)return;const panel=$('#case-notes-panel'),opening=panel.classList.contains('hidden');panel.classList.toggle('hidden',!opening);if(opening)await loadCaseNotes()}
function ensurePhenotypeDialog(){
  let dialog=$('#phenotype-dialog');
  if(dialog)return dialog;
  document.body.insertAdjacentHTML('beforeend',`<dialog id="phenotype-dialog" class="install-review phenotype-dialog" aria-labelledby="phenotype-dialog-title"><div class="phenotype-dialog-heading"><div><p class="kicker">EXPERIMENTAL PHENOTYPE PRIORITIZATION</p><h2 id="phenotype-dialog-title">Phenotype prioritization</h2><p>Compare patient findings with HPO disease profiles and review report evidence in one candidate list.</p></div><button type="button" data-close-phenotypes aria-label="Close phenotype prioritization">${prototypeIcon('close')}</button></div><div class="phenotype-dialog-body" data-phenotype-body><p class="phenotype-loading">Loading phenotype profile...</p></div></dialog>`);
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
  return`<div class="phenotype-chips">${terms.map(term=>`<span><b>${escapeHtml(term.label)}</b><small>${escapeHtml(term.id)}</small><button type="button" data-phenotype-kind="${kind}" data-remove-phenotype="${escapeHtml(term.id)}" aria-label="Remove ${escapeHtml(term.label)}">${prototypeIcon('close')}</button></span>`).join('')}</div>`
}
function renderPhenotypeDialog(){
  const body=$('#phenotype-dialog [data-phenotype-body]');
  if(!body)return;
  if(!phenotypeProfile){body.innerHTML='<p class="phenotype-loading">Loading phenotype profile...</p>';return}
  if(resourceStates.hpo&&!resourceStates.hpo.ready){
    const status=resourceStates.hpo.label||'Not installed';
    body.innerHTML=`<section class="phenotype-unavailable"><div class="phenotype-info-icon">${prototypeIcon('info')}</div><h3>Install Human Phenotype Ontology data</h3><p>Local term search and phenotype similarity require the managed HPO release. The ontology, disease annotations, and disease-gene associations install together as one data source.</p><p><strong>Status:</strong> ${escapeHtml(status)}</p><button type="button" class="primary" data-install-hpo>Open Data sources</button></section>`;
    return
  }
  const sampleNames=phenotypeProfile.sampleNames||[],release=phenotypeProfile.hpoRelease||'installed release',observed=phenotypeProfile.observed||[],excluded=phenotypeProfile.excluded||[],hasObserved=observed.length>0;
  const sampleControl=sampleNames.length>1?`<label class="phenotype-sample-picker"><span>Patient sample</span><select data-phenotype-sample><option value="">Choose a sample</option>${sampleNames.map(name=>`<option value="${escapeHtml(name)}" ${name===phenotypeSampleName?'selected':''}>${escapeHtml(name)}</option>`).join('')}</select><small>Only exact ALT alleles carried by this sample contribute to report overlap.</small></label>`:sampleNames.length===1?`<div class="phenotype-sample-picker fixed"><span>Patient sample</span><strong>${escapeHtml(sampleNames[0])}</strong><small>Exact carried ALT alleles are used for report overlap.</small></div>`:`<div class="phenotype-sample-picker unavailable"><span>Report overlap unavailable</span><small>This report has no sample genotype columns. Phenotype similarity can still be calculated.</small></div>`;
  body.innerHTML=`${sampleControl}<section class="phenotype-profile-editor" aria-label="Patient phenotype profile"><div class="phenotype-profile-column"><h3>Observed findings</h3><p>Phenotypic abnormalities present in the patient.</p>${phenotypeTermChips('observed',observed)}</div><div class="phenotype-profile-column"><h3>Explicitly absent findings</h3><p>Only add abnormalities that were assessed and not found.</p>${phenotypeTermChips('excluded',excluded)}</div></section><section class="phenotype-term-search"><label><span>Add as</span><select id="phenotype-presence"><option value="observed">Observed</option><option value="excluded">Explicitly absent</option></select></label><label class="phenotype-search-field"><span>HPO phenotypic abnormality</span><input type="search" data-phenotype-search role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="phenotype-search-results" placeholder="Search by feature, synonym, or HP identifier" autocomplete="off"><div id="phenotype-search-results" class="phenotype-search-results" data-phenotype-search-results role="listbox" aria-label="Matching HPO terms"></div></label></section><div class="phenotype-actions"><p data-phenotype-message>${escapeHtml(phenotypeMessage)}</p><label class="phenotype-online-option"><input type="checkbox" data-monarch-enrichment ${phenotypeOnlineConsent?'checked':''}><span>Add Monarch gene suggestions</span><small>Sends observed HPO IDs and returns up to 50 genes.</small></label>${hasObserved?'':`<button type="button" data-explore-report-phenotypes ${phenotypeSampleName?'':'disabled'}>Explore report associations</button>`}<button type="button" data-clear-phenotypes>Clear profile</button><button type="button" class="primary" data-rank-phenotypes ${hasObserved?'':'disabled'}>Prioritize candidates</button></div><section class="phenotype-ranking" data-phenotype-ranking></section><p class="phenotype-attribution">Uses Human Phenotype Ontology release ${escapeHtml(release)}. <a href="https://human-phenotype-ontology.github.io/license.html" target="_blank" rel="noopener noreferrer">HPO license and attribution</a>.</p>`;
  renderPhenotypeResults()
}
function renderPhenotypeSearchResults(){
  const host=$('#phenotype-dialog [data-phenotype-search-results]'),input=$('#phenotype-dialog [data-phenotype-search]');
  if(!host)return;
  if(phenotypeSearchActiveIndex>=phenotypeSearchResults.length)phenotypeSearchActiveIndex=phenotypeSearchResults.length-1;
  host.innerHTML=phenotypeSearchResults.length?phenotypeSearchResults.map((term,index)=>`<button id="phenotype-search-option-${index}" type="button" role="option" aria-selected="${index===phenotypeSearchActiveIndex}" class="${index===phenotypeSearchActiveIndex?'active':''}" data-phenotype-search-result="${escapeHtml(term.id)}"><span><strong>${escapeHtml(term.label)}</strong><small>${escapeHtml(term.id)}</small></span>${term.synonyms?.length?`<em>${escapeHtml(term.synonyms.join('; '))}</em>`:''}</button>`).join(''):'';
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
  return`<article class="phenotype-candidate ${overlap.hasOverlap?'report-supported':''}"><div class="phenotype-candidate-heading"><div><span class="phenotype-candidate-rank">#${rank.toLocaleString()} ${rankLabel}</span><h4>${escapeHtml(disease.diseaseName)}</h4><p>${escapeHtml(disease.diseaseId)}${geneSymbols.length?` <span aria-hidden="true">·</span> ${geneSymbols.slice(0,6).map(symbol=>escapeHtml(symbol)).join(', ')}${geneSymbols.length>6?` +${geneSymbols.length-6}`:''}`:''}</p></div>${bestOnline?`<span class="phenotype-source-badge" title="Optional Monarch phenotype-to-gene result; not combined with the local score">Monarch #${Number(bestOnline.rank).toLocaleString()}</span>`:''}</div><div class="phenotype-evidence-summary"><div><span>${phenotypeLabel}</span><strong>${phenotypeValue}</strong><small>${phenotypeNote}</small></div><div class="${overlap.hasOverlap?'supporting':''}"><span>${reportLabel}</span><strong>${reportValue}</strong><small>${reportNote}</small></div><div class="${!reportOnly&&Number(disease.conflictScore)>0?'conflicting':''}"><span>${conflictLabel}</span><strong>${conflictValue}</strong><small>${conflictNote}</small></div></div><details class="phenotype-candidate-details"><summary>Review evidence</summary><div class="phenotype-candidate-evidence">${matchRows?`<section><h5>${reportOnly?'HPO disease profile':'Phenotype evidence'}</h5>${matchRows}</section>`:''}${reportRows?`<section><h5>Report evidence</h5>${reportRows}<p>Uses exact ALT alleles carried by the selected sample, literal VCF PASS, and the report's representative effect. This is not evidence of pathogenicity or causality.</p></section>`:`<section><h5>Report evidence</h5><p>${hasReportSample?'No carried ALT with a representative HIGH or MODERATE effect overlapped a Mendelian disease gene.':'Choose a patient sample to evaluate exact carried-ALT overlap.'}</p></section>`}${associationRows?`<section><h5>Gene-disease relationships</h5>${associationRows}${associations.length>16?`<p>${associations.length-16} additional associations are not shown.</p>`:''}</section>`:''}${!reportOnly&&Number(disease.conflictScore)>0?`<section><h5>Potential conflicts</h5><p>Observed or explicitly absent findings conflict with this disease profile at ${Number(disease.conflictScore).toFixed(0)}/100 similarity. This signal does not change the phenotype order.${disease.conflictFrequencyComplete?'':' Some HPO disease-feature frequencies were unavailable, so those matches are unweighted.'}</p></section>`:''}</div></details></article>`
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
    host.innerHTML=`<div class="phenotype-ranking-heading"><div><span class="phenotype-results-kicker">REPORT-ONLY EXPLORATION</span><h3>Candidate evidence</h3><p>${Number(phenotypeExploration.associatedDiseases||0).toLocaleString()} HPO disease profiles have Mendelian gene associations overlapping ${Number(phenotypeExploration.reportGeneCount||0).toLocaleString()} genes with exact ALT alleles carried by ${escapeHtml(phenotypeExploration.sampleName)}. This uses literal PASS and the representative transcript effect, but does not evaluate phenotype fit, inheritance, allele frequency, pathogenicity, causality, or disease likelihood.</p></div></div><div class="phenotype-candidate-list">${shown.map(disease=>phenotypeCandidateCard(disease,{reportOnly:true})).join('')}</div>${shown.length<diseases.length?`<button type="button" class="phenotype-more" data-more-phenotype-results>Show 100 more</button>`:''}`;
    return
  }
  if(!ranking){host.innerHTML='<div class="phenotype-ranking-empty"><h3>No candidate comparison yet</h3><p>Add at least one observed finding and prioritize candidates. If no phenotype profile is available, report associations can be explored separately.</p></div>';return}
  let diseases=[...(ranking.diseases||[])];
  diseases.sort((left,right)=>comparePhenotypeCandidates(left,right,phenotypeResultSort==='overlap'));
  const shown=diseases.slice(0,phenotypeResultLimit);
  const online=ranking.onlineEnrichment,onlineGenes=online?.genes||[],onlineByGene=new Map(onlineGenes.map(gene=>[String(gene.symbol||'').toUpperCase(),gene])),localGeneSymbols=new Set(diseases.flatMap(disease=>(disease.genes||[]).map(gene=>String(gene.symbol||'').toUpperCase()))),additionalOnline=onlineGenes.filter(gene=>!localGeneSymbols.has(String(gene.symbol||'').toUpperCase()));
  const onlineNote=online?`<div class="phenotype-source-note"><b>Monarch suggestions integrated</b><span>Matching genes are labeled in candidate cards. Monarch returned ${onlineGenes.length.toLocaleString()} of at most ${Number(online.resultLimit||50).toLocaleString()} suggestions; its score is not combined with local phenotype similarity.</span></div>${additionalOnline.length?`<details class="phenotype-online-supplement"><summary>Additional Monarch gene suggestions (${additionalOnline.length.toLocaleString()})</summary><div>${additionalOnline.map(gene=>`<span><b>${escapeHtml(gene.symbol)}</b><small>#${Number(gene.rank).toLocaleString()} · ${Number(gene.score).toFixed(3)}</small></span>`).join('')}</div></details>`:''}`:ranking.onlineError?`<p class="phenotype-online-error">${escapeHtml(ranking.onlineError)} Local HPO comparison completed normally.</p>`:'';
  const overlapNote=ranking.sampleName?`Report evidence uses exact genotypes for ${escapeHtml(ranking.sampleName)}, literal PASS, Mendelian gene associations, and representative effects. It does not evaluate inheritance, allele frequency, pathogenicity, or causality.`:'No patient sample was selected, so report evidence was not evaluated.';
  host.innerHTML=`<div class="phenotype-ranking-heading"><div><span class="phenotype-results-kicker">UNIFIED CANDIDATE VIEW</span><h3>Candidate evidence</h3><p>${Number(ranking.evaluatedDiseases||diseases.length).toLocaleString()} local HPO disease profiles compared by patient-to-disease Lin similarity. Unrecorded disease findings are treated as unknown; explicitly absent findings are shown separately as potential conflicts. This is an experimental evidence order, not a diagnostic probability or validated clinical ranking. ${overlapNote}</p></div><label><span>Order</span><select data-phenotype-sort><option value="phenotype" ${phenotypeResultSort==='phenotype'?'selected':''}>Phenotype match</option><option value="overlap" ${phenotypeResultSort==='overlap'?'selected':''}>Group by report support</option></select></label></div>${onlineNote}<div class="phenotype-candidate-list">${phenotypeCandidateList(shown,{onlineByGene,hasReportSample:Boolean(ranking.sampleName)})}</div>${shown.length<diseases.length?`<button type="button" class="phenotype-more" data-more-phenotype-results>Show 100 more</button>`:''}`
}
async function openPhenotypeDialog(){
  if(!currentResultRun)return;
  const dialog=ensurePhenotypeDialog();
  phenotypeDialogRunId=currentResultRun.id;phenotypeProfile=null;phenotypeExploration=null;phenotypeSampleName='';phenotypeMessage='';phenotypeOnlineConsent=false;phenotypeResultLimit=100;phenotypeSearchResults=[];phenotypeSearchActiveIndex=-1;phenotypeSaveRevision++;
  renderPhenotypeDialog();dialog.showModal();
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
function selectionCount(){return selectionMode==='filtered'?Math.max(0,resultTotal-excludedFilteredAlleles.size):selectedAlleles.size}
function displayedSearchEvidenceColumns(search=$('#search').value.trim()){return search?[...visibleEvidence].sort((a,b)=>a-b):[]}
function currentResultFilterSignature(){const search=$('#search').value.trim();return JSON.stringify({search,evidenceColumns:displayedSearchEvidenceColumns(search),...resultFilterParameters()})}
function currentResultCountSignature(run=currentResultRun){return JSON.stringify([run?.id||'',resultView,currentResultFilterSignature()])}
function hasActiveResultQuery(){const filters=resultFilterParameters();return Boolean($('#search').value.trim()||filters.filterRules.length||filters.evidenceFilters.length)}
function updateResultPageStatus(){const status=$('#result-page-status');if(!status)return;if(resultQueryError){status.textContent=resultQueryError;status.classList.add('error');return}status.classList.remove('error');if(resultLoading){const loaded=variants.length?`${variants.length.toLocaleString()} loaded · `:'';status.innerHTML=`${escapeHtml(loaded)}<i class="result-query-spinner" aria-hidden="true"></i>${escapeHtml(resultOperation||'Loading…')}`;return}status.textContent=resultTotal===0&&hasActiveResultQuery()?'No matching variants':`${variants.length.toLocaleString()} of ${resultTotal.toLocaleString()}`}
function scheduleResultSearch(){clearTimeout(resultSearchTimer);resultRequestController?.abort();resultRequestGeneration++;resultPageMemory.clear();resultQueryError='';resultOperation='Searching…';resultLoading=true;updateResultPageStatus();updateResultScrollState();resultSearchTimer=setTimeout(()=>{if(currentResultRun)openCompletedRun(currentResultRun,0);else{resultLoading=false;resultOperation='';updateResultPageStatus()}},250)}
function updateSelectionControls(){const count=selectionCount(),allFiltered=selectionMode==='filtered',candidateButton=$('#candidate-selected'),candidateLabel=$('#candidate-selected-label'),removeCandidates=resultView==='candidates'||!allFiltered&&count>0&&[...selectedAlleles].every(id=>candidateAlleles.has(id));$('#selection-actions').classList.toggle('hidden',count===0);candidateButton?.classList.toggle('hidden',count===0);if(candidateButton){const action=`${removeCandidates?'Remove':'Add'} ${count.toLocaleString()} selected variant${count===1?'':'s'} ${removeCandidates?'from':'to'} candidates`;candidateLabel.textContent=`${removeCandidates?'Remove from':'Add to'} candidates (${count.toLocaleString()})`;candidateButton.title=action;candidateButton.setAttribute('aria-label',action)}$('#export-selected-genes-label').textContent=count?`Export genes (${count.toLocaleString()})`:'Export genes';$('#export-selected-rows-label').textContent=count?`Export rows (${count.toLocaleString()})`:'Export rows';if(!count){$('#selection-actions-menu').classList.add('hidden');$('#selection-actions-toggle').setAttribute('aria-expanded','false')}}
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
function likelyNumericEvidenceField(field){const name=String(field?.fieldPath||'').toLowerCase();return['integer','number'].includes(field?.valueType)||/(^|_)(score|rankscore|phred|raw|af|faf|ac|an|nhomalt|count|frequency|percentile|distance|depth|dp|gq|mq|fs|sor|qd)(_|$)/.test(name)||/(phylop|gerp|spliceai|cadd|revel|primateai|alphamissense)/.test(name)&&!/(pred|prediction|class|label|id)$/.test(name)}
function filterColumnDefinition(value){if(value?.startsWith('evidence:')){const index=Number(value.slice(9)),field=resultFieldCatalog[index],presentation=field?evidenceFieldPresentation(field):null;return field?{key:value,label:`${resourceTitle(field.sourceId)} · ${presentation.label}`,type:likelyNumericEvidenceField(field)?'number':field.valueType==='boolean'?'boolean':'text',field,index}:null}return coreFilterColumns.find(column=>column.key===value)||null}
function resultFilterRules(){return[...$('#filter-rules').querySelectorAll('.filter-rule')].map(row=>({column:row.querySelector('[data-filter-column]').value,operator:row.querySelector('[data-filter-operator]').value,value:row.querySelector('[data-filter-value]').value.trim()})).filter(rule=>rule.column&&rule.operator&&rule.value!=='')}
function resultFilterParameters(){const filterRules=[],evidenceFilters=[];resultFilterRules().forEach(rule=>{const definition=filterColumnDefinition(rule.column);if(!definition)return;if(definition.field)evidenceFilters.push({index:definition.index,operator:rule.operator,value:rule.value,value2:''});else filterRules.push(rule)});return{filterRules,evidenceFilters}}
function filterColumnChoices(){const choices=coreFilterColumns.map(column=>{const presentation=coreColumnPresentation(column.key,column.label);return{key:column.key,label:humanReadableColumnNames?presentation.readableLabel:column.key,raw:column.key,source:'Core annotation',description:presentation.description}});selectableEvidenceEntries().forEach(({field,index})=>{const presentation=evidenceFieldPresentation(field);choices.push({key:`evidence:${index}`,label:humanReadableColumnNames?presentation.label:field.fieldPath,raw:field.fieldPath,source:resourceTitle(field.sourceId||'Other evidence'),description:presentation.description})});return choices}
function filterColumnPicker(selected){const choices=filterColumnChoices(),current=choices.find(choice=>choice.key===selected)||choices[0],groups=new Map();choices.forEach(choice=>{if(!groups.has(choice.source))groups.set(choice.source,[]);groups.get(choice.source).push(choice)});const options=[...groups.entries()].map(([source,items])=>`<section data-filter-column-option-group><strong>${escapeHtml(source)}</strong>${items.map(choice=>`<button type="button" role="option" data-filter-column-option="${escapeHtml(choice.key)}" data-filter-column-search-text="${escapeHtml(`${source} ${choice.label} ${choice.raw} ${choice.description}`.toLowerCase())}" aria-selected="${choice.key===current.key}"><span class="filter-column-option-copy"><strong data-filter-column-option-label>${escapeHtml(choice.label)}</strong><small>${escapeHtml(choice.description)}</small></span><code>${escapeHtml(choice.raw)}</code></button>`).join('')}</section>`).join('');return`<div class="filter-column-picker"><input type="hidden" data-filter-column value="${escapeHtml(current.key)}"><button type="button" data-filter-column-toggle aria-haspopup="listbox" aria-expanded="false" title="${escapeHtml(current.description)}"><span data-filter-column-label>${escapeHtml(current.label)}</span><svg class="ui-icon filter-column-chevron" aria-hidden="true"><use href="#icon-chevron-down"></use></svg></button><div class="filter-column-options hidden" role="listbox"><input type="search" data-filter-column-search aria-label="Search filter columns" placeholder="Search columns, sources, descriptions, or raw keys"><div class="filter-column-option-list">${options}</div></div></div>`}
function filterValueControl(definition,value){if(definition?.type==='boolean')return`<select data-filter-value aria-label="Filter value"><option value="true" ${value==='true'?'selected':''}>Yes</option><option value="false" ${value==='false'?'selected':''}>No</option></select>`;const placeholder=definition?.key==='gene'?'BRCA1, BRCA2, TP53':definition?.type==='number'?'Enter a number':'Enter a value';return`<input data-filter-value value="${escapeHtml(value||'')}" placeholder="${escapeHtml(placeholder)}" ${definition?.type==='number'?'inputmode="decimal"':''}>`}
function defaultFilterOperator(definition){const name=`${definition?.key||''} ${definition?.field?.sourceId||''} ${definition?.field?.fieldPath||''}`.toLowerCase();if(definition?.type==='boolean')return'equals';if(definition?.type==='number'){if(/(^|[^a-z])(af|faf)([^a-z]|$)|frequency|sift/.test(name))return'lte';if(/quality|score|phred|phylop|gerp|revel|cadd|spliceai|primateai|alphamissense/.test(name))return'gte';return'equals'}if(definition?.key==='gene')return'in';if(/consequence|phenotype|condition|disease|significance/.test(name))return'contains';return'equals'}
function filterOperatorOptions(definition,selected){const allowed=new Set(definition?.type==='number'?['equals','not_equals','gt','gte','lt','lte']:definition?.type==='boolean'?['equals','not_equals']:['equals','not_equals','contains','not_contains','in']),choice=allowed.has(selected)?selected:defaultFilterOperator(definition);return filterOperators.filter(([value])=>allowed.has(value)).map(([value,label])=>`<option value="${value}" ${choice===value?'selected':''}>${escapeHtml(label)}</option>`).join('')}
function addFilterRule(rule={column:'gene',operator:'in',value:''},render=true){const definition=filterColumnDefinition(rule.column)||coreFilterColumns.find(column=>column.key==='gene');$('#filter-rules').insertAdjacentHTML('beforeend',`<div class="filter-rule">${filterColumnPicker(definition.key)}<select data-filter-operator aria-label="Filter comparison">${filterOperatorOptions(definition,rule.operator)}</select><span class="filter-rule-value">${filterValueControl(definition,rule.value)}</span><button type="button" data-remove-filter aria-label="Remove filter">×</button></div>`);if(render)bindFilterRule($('#filter-rules').lastElementChild)}
function filterFilterColumnOptions(picker,query){const normalized=query.trim().toLowerCase();picker.querySelectorAll('[data-filter-column-option-group]').forEach(group=>{const options=[...group.querySelectorAll('[data-filter-column-option]')];options.forEach(option=>option.classList.toggle('hidden',Boolean(normalized)&&!option.dataset.filterColumnSearchText.includes(normalized)));group.classList.toggle('hidden',options.every(option=>option.classList.contains('hidden')))})}
function closeFilterColumnPicker(picker){picker.querySelector('.filter-column-options').classList.add('hidden');picker.querySelector('[data-filter-column-toggle]').setAttribute('aria-expanded','false')}
function bindFilterRule(row){const picker=row.querySelector('.filter-column-picker'),value=picker.querySelector('[data-filter-column]'),toggle=picker.querySelector('[data-filter-column-toggle]'),menu=picker.querySelector('.filter-column-options'),search=picker.querySelector('[data-filter-column-search]');toggle.addEventListener('click',()=>{const open=menu.classList.contains('hidden');document.querySelectorAll('.filter-column-picker').forEach(other=>{if(other!==picker)closeFilterColumnPicker(other)});menu.classList.toggle('hidden',!open);toggle.setAttribute('aria-expanded',String(open));if(open){search.value='';filterFilterColumnOptions(picker,'');requestAnimationFrame(()=>search.focus())}});search.addEventListener('input',()=>filterFilterColumnOptions(picker,search.value));search.addEventListener('keydown',event=>{event.stopPropagation();if(event.key==='Escape'){event.preventDefault();closeFilterColumnPicker(picker);toggle.focus()}else if(event.key==='Enter'){const option=picker.querySelector('[data-filter-column-option]:not(.hidden)');if(option){event.preventDefault();option.click()}}});menu.addEventListener('click',event=>{const option=event.target.closest('[data-filter-column-option]');if(!option)return;value.value=option.dataset.filterColumnOption;const choice=filterColumnChoices().find(item=>item.key===value.value),definition=filterColumnDefinition(value.value),previous=row.querySelector('[data-filter-value]').value;toggle.querySelector('[data-filter-column-label]').textContent=option.querySelector('[data-filter-column-option-label]').textContent;toggle.title=choice?.description||'';picker.querySelectorAll('[data-filter-column-option]').forEach(item=>item.setAttribute('aria-selected',String(item===option)));closeFilterColumnPicker(picker);row.querySelector('[data-filter-operator]').innerHTML=filterOperatorOptions(definition,defaultFilterOperator(definition));row.querySelector('.filter-rule-value').innerHTML=filterValueControl(definition,previous);filterRulesChanged()});row.querySelector('[data-remove-filter]').addEventListener('click',event=>{event.stopPropagation();row.remove();if(!$('#filter-rules').children.length)addFilterRule();filterRulesChanged()})}
function validateResultFilters(){for(const rule of resultFilterRules()){if(numericFilterOperators.has(rule.operator)&&(!Number.isFinite(Number(rule.value))||rule.value===''))return`“${filterColumnDefinition(rule.column)?.label||rule.column}” needs a valid number for ${rule.operator==='gte'?'≥':rule.operator==='lte'?'≤':rule.operator==='gt'?'>':'<'}`;}return''}
function renderFilterRules(rules=resultFilterRules()){const host=$('#filter-rules');host.innerHTML='';(rules.length?rules:[{column:'gene',operator:'in',value:''}]).forEach(rule=>addFilterRule(rule,false));host.querySelectorAll('.filter-rule').forEach(bindFilterRule)}
function filterRulesChanged(){resultPageMemory.clear();if(selectionMode==='filtered')clearVariantSelection(true);$('#filter-message').textContent='Filters changed — apply to update results'}
function clearResultFilters(refresh=true){renderFilterRules([{column:'gene',operator:'in',value:''}]);$('#filter-message').textContent='';if(refresh&&currentResultRun)openCompletedRun(currentResultRun,0)}
function savedFilterPresets(){try{const value=JSON.parse(localStorage.getItem(FILTER_PRESET_STORAGE_KEY)||'[]');return Array.isArray(value)?value.slice(0,50):[]}catch{return[]}}
function refreshFilterPresetSelector(selected=''){const presets=savedFilterPresets(),selector=$('#saved-filter-presets');selector.innerHTML='<option value="">Choose a saved filter…</option>'+presets.map((preset,index)=>`<option value="${index}" ${String(index)===String(selected)?'selected':''}>${escapeHtml(preset.name)}</option>`).join('')}
function presetRules(){return resultFilterRules().map(rule=>{const definition=filterColumnDefinition(rule.column);return definition?.field?{column:'evidence',operator:rule.operator,value:rule.value,field:{scope:definition.field.scope,sourceId:definition.field.sourceId,fieldPath:definition.field.fieldPath}}:rule})}
function saveFilterPreset(){const rules=presetRules();if(!rules.length){$('#filter-message').textContent='Add at least one complete filter before saving';return}const name=prompt('Saved filter name');if(!name?.trim())return;const presets=savedFilterPresets(),clean=name.trim().slice(0,80),existing=presets.findIndex(preset=>preset.name.toLowerCase()===clean.toLowerCase()),preset={name:clean,rules};if(existing>=0)presets[existing]=preset;else presets.push(preset);localStorage.setItem(FILTER_PRESET_STORAGE_KEY,JSON.stringify(presets.slice(0,50)));refreshFilterPresetSelector(existing>=0?existing:presets.length-1);$('#filter-message').textContent=`Saved “${clean}” for all reports`}
function loadFilterPreset(){const selected=$('#saved-filter-presets').value;if(selected==='')return;const index=Number(selected),preset=savedFilterPresets()[index];if(!preset)return;let unavailable=0;const rules=preset.rules.map(rule=>{if(rule.column!=='evidence')return rule;const fieldIndex=resultFieldCatalog.findIndex(field=>field.scope===rule.field?.scope&&field.sourceId===rule.field?.sourceId&&field.fieldPath===rule.field?.fieldPath);if(fieldIndex<0){unavailable++;return null}return{column:`evidence:${fieldIndex}`,operator:rule.operator,value:rule.value}}).filter(Boolean);renderFilterRules(rules);if(selectionMode==='filtered')clearVariantSelection(true);$('#filter-message').textContent=unavailable?`${unavailable} saved database field${unavailable===1?' is':'s are'} unavailable in this report`:`Loaded “${preset.name}”`}
function deleteFilterPreset(){const selected=$('#saved-filter-presets').value;if(selected==='')return;const index=Number(selected),presets=savedFilterPresets();if(!presets[index])return;const [removed]=presets.splice(index,1);localStorage.setItem(FILTER_PRESET_STORAGE_KEY,JSON.stringify(presets));refreshFilterPresetSelector();$('#filter-message').textContent=`Deleted “${removed.name}”`}
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
  $('#head').innerHTML=`<th class="selection-cell"><input id="result-select-all-checkbox" type="checkbox" aria-label="Select all filtered variants" title="Select or clear all filtered variants"></th><th class="candidate-cell"><button id="candidate-all" type="button" class="candidate-toggle candidate-column-heading ${allVisibleCandidates?'active':''}" aria-label="${candidateAction}" title="${candidateAction}">${prototypeIcon('star')}<span class="legacy-icon">${allVisibleCandidates?'★':'☆'}</span></button></th>`+shown.map(([key,label,description,,sourceId])=>{const tooltip=resultColumnTooltip(key,description,sourceId),priority=resultSorts.findIndex(sort=>sort.key===key),sort=priority<0?null:resultSorts[priority],indicator=sort?`${resultSorts.length>1?priority+1:''}${sort.direction==='asc'?'▲':'▼'}`:'↕',ariaSort=priority===0?(sort.direction==='asc'?'ascending':'descending'):'none';return`<th title="${escapeHtml(tooltip)}" aria-sort="${ariaSort}"><button type="button" class="column-sort" data-sort-column="${escapeHtml(key)}" aria-label="${escapeHtml(`${label}. ${tooltip} Click to sort; Shift-click to add another sort column.`)}"><span>${escapeHtml(label)}</span><b aria-hidden="true">${indicator}</b></button></th>`}).join('');
  $('#rows').innerHTML=variants.length?variants.map(row=>{const candidate=candidateAlleles.has(row.alleleId),selected=allFiltered?!excludedFilteredAlleles.has(row.alleleId):selectedAlleles.has(row.alleleId),classes=[row.alleleId===selectedAlleleId?'selected-variant':'',selected?'selection-active':''].filter(Boolean).join(' ');return`<tr ${row.alleleId?`tabindex="0" data-allele-id="${escapeHtml(row.alleleId)}" aria-label="Open details for ${escapeHtml(`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`)}"`:''} class="${classes}"><td class="selection-cell">${row.alleleId?`<input type="checkbox" data-select-allele="${escapeHtml(row.alleleId)}" aria-label="Select ${escapeHtml(`${row.chromosome}:${row.position} ${row.reference}>${row.alternate}`)}" ${selected?'checked':''}>`:''}</td><td class="candidate-cell">${row.alleleId?`<button type="button" class="candidate-toggle ${candidate?'active':''}" data-toggle-candidate="${escapeHtml(row.alleleId)}" aria-label="${candidate?'Remove from':'Add to'} candidates" title="${candidate?'Remove from':'Add to'} candidates">${prototypeIcon('star')}<span class="legacy-icon">${candidate?'★':'☆'}</span></button>`:''}</td>${shown.map(([key])=>{const value=resultColumnValue(row,key);return key==='impact'?`<td><span class="impact impact-${String(value||'').toLowerCase().replace(/[^a-z0-9_-]/g,'')}">${escapeHtml(value)}</span></td>`:`<td>${escapeHtml(value)}</td>`}).join('')}</tr>`}).join(''):emptyMessage?`<tr class="empty-result-row"><td colspan="${Math.max(2,shown.length+2)}">${escapeHtml(emptyMessage)}</td></tr>`:'';
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
function dbnsfpPredictionValue(field,value){if(value==='.'||value==='')return'Not reported';const labels={SIFT_pred:{D:'Deleterious',T:'Tolerated'},Polyphen2_HDIV_pred:{D:'Probably damaging',P:'Possibly damaging',B:'Benign'},Polyphen2_HVAR_pred:{D:'Probably damaging',P:'Possibly damaging',B:'Benign'},AlphaMissense_pred:{B:'Benign',A:'Uncertain',P:'Pathogenic'},PrimateAI_pred:{D:'Damaging',T:'Tolerated'},VEP_canonical:{YES:'Yes'},GENCODE_basic:{Y:'Yes'}};return labels[field]?.[value]||value}
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
  const search=`<div class="column-menu-searchbar"><input class="column-menu-search" type="search" data-column-search aria-label="Search displayed columns" placeholder="Search columns, sources, descriptions, or raw keys"></div>`;
  const preference=`<div class="column-menu-toolbar"><label class="column-name-preference"><input type="checkbox" data-human-readable-columns ${humanReadableColumnNames?'checked':''}><span><strong>Human-readable column names</strong><small>Turn off to show raw report field keys.</small></span></label><button type="button" data-restore-default-columns>Restore defaults</button></div>`;
  const core=columnGroups.map(group=>`<fieldset data-column-group><legend><label><input type="checkbox" data-column-group-toggle><span>${escapeHtml(group.label)}</span></label></legend>${group.columns.map(([key,label])=>{const presentation=coreColumnPresentation(key,label);return`<label title="${escapeHtml(presentation.description)}"><input type="checkbox" data-key="${key}" ${visible.has(key)?'checked':''}><span class="column-field-copy"><strong>${escapeHtml(presentation.label)}</strong><small>${escapeHtml(presentation.description)}</small><code>${escapeHtml(key)}</code></span></label>`}).join('')}</fieldset>`).join('');
  const dynamic=[...groups.entries()].map(([source,fields])=>`<fieldset class="evidence-column-group" data-column-group><legend><label><input type="checkbox" data-column-group-toggle><span>${escapeHtml(resourceTitle(source))}</span></label></legend>${fields.map(({field,index})=>{const presentation=evidenceFieldPresentation(field),label=humanReadableColumnNames?presentation.label:field.fieldPath;return`<label title="${escapeHtml(presentation.description)}"><input type="checkbox" data-evidence-index="${index}" ${visibleEvidence.has(index)?'checked':''}><span class="column-field-copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(presentation.description)}</small><code>${escapeHtml(field.fieldPath)} · ${escapeHtml(field.valueType||'unknown')}</code></span></label>`}).join('')}</fieldset>`).join('');
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
function renderProfiles(){const ordered=[...profiles].sort((a,b)=>a.id==='wgs'?-1:b.id==='wgs'?1:0),preferred=ordered.some(profile=>profile.id==='wgs')?'wgs':ordered[0]?.id;profiles=ordered;$('#profile').innerHTML=ordered.map(profile=>`<option value="${escapeHtml(profile.id)}" ${profile.id===preferred?'selected':''}>${escapeHtml(profile.name)}${profile.id==='wgs'?' (recommended)':''}</option>`).join('')+'<option value="custom">Custom</option>';const host=$('#profile-install-actions');host.innerHTML=ordered.map(profile=>{const label=profile.name,names=['Core annotation data',...profile.sourceIds.map(id=>sources.find(source=>source.id===id)?.name||id)];return`<article><strong>${escapeHtml(label)}${profile.id==='wgs'?' · Recommended':''}</strong><small>${escapeHtml(names.join(' · '))}</small><button type="button" data-profile-install="${escapeHtml(profile.id)}">Install ${escapeHtml(label)}</button></article>`}).join('');host.querySelectorAll('[data-profile-install]').forEach(button=>button.addEventListener('click',()=>showProfileInstallReview(button.dataset.profileInstall)))}
function sourceAvailabilityLabel(source){if(source.delivery==='user-supplied-licensed')return'Licensed files required';if(source.delivery==='managed-public-noncommercial')return'Non-commercial use';if(source.delivery==='adapter-required')return'Adapter pending';if(source.delivery==='catalog-pending')return'Catalog pending';return'Installer pending'}
function sourceLicenseNote(source){if(source.id==='hpo')return'<small class="source-license-note">Uses a versioned Human Phenotype Ontology release. <a href="https://human-phenotype-ontology.github.io/license.html" target="_blank" rel="noopener noreferrer">License and attribution</a>.</small>';if(source.delivery==='managed-public-noncommercial')return'<small class="source-license-note">Non-commercial use; commercial use requires a CADD license.</small>';if(source.delivery==='user-supplied-licensed')return`<small class="source-license-note">Import files obtained under your ${escapeHtml(source.name)} license.</small>`;return''}
function sourceInstallRank(source){return resourcePlan.resources.some(resource=>resource.id===source.id&&resource.state==='missing')?0:1}
function orderedCatalogSources(){return sources.filter(source=>source.id!=='fastvep').map((source,index)=>({source,index})).sort((a,b)=>sourceInstallRank(a.source)-sourceInstallRank(b.source)||a.index-b.index).map(item=>item.source)}
function renderWizardSources(){const recommended=new Set(selectedProfile()?.sourceIds||[]),container=$('#wizard-sources'),availableCatalog=orderedCatalogSources().filter(source=>source.fastvepSource&&resourcePlan.resources.some(item=>item.id===source.id&&item.state==='missing'));container.innerHTML=availableCatalog.map(source=>{const state=resourceStates[source.id],ready=Boolean(state?.ready),isRecommended=recommended.has(source.id),badge=isRecommended?'<small class="profile-badge">Profile</small>':'';return`<label class="source-option ${ready?'':'source-unavailable'}"><input type="checkbox" data-source="${escapeHtml(source.id)}" ${ready&&isRecommended?'checked':''} ${ready?'':'disabled'}><span><strong>${escapeHtml(source.name)} ${badge}</strong><small>${escapeHtml(source.purpose)}</small></span><em data-resource-state="${escapeHtml(source.id)}">${ready?'Installed':'Not installed'}</em></label>`}).join('');container.querySelectorAll('input[data-source]').forEach(input=>input.addEventListener('change',sourceSelectionChanged))}
function sourceSelectionChanged(event){const input=event.target;if(input.checked&&['gnomad','gnomad-genomes'].includes(input.dataset.source)){const other=$(`#wizard-sources input[data-source="${input.dataset.source==='gnomad'?'gnomad-genomes':'gnomad'}"]`);if(other)other.checked=false}$('#profile').value='custom';$('#wizard-sources .profile-badge').forEach(badge=>badge.remove());updateWizardReadiness()}
function updateWizardReadiness(){const host=$('#wizard-readiness');if(!host)return;const profile=selectedProfile(),selected=enabledSourceIds(),missing=(profile?.sourceIds||[]).filter(id=>!resourceStates[id]?.ready);let tone='ready',title='',detail='';if(!lastSetupReady){tone='blocked';title='Local annotation is not ready';detail='Install the GRCh38 reference and transcript cache before starting an annotation.'}else if(profile&&missing.length){tone='partial';title=`${selected.length} available profile source${selected.length===1?'':'s'} selected`;detail=`${missing.length} profile source${missing.length===1?' is':'s are'} not installed. You can continue with the available sources or manage data sources.`}else if(selected.length){title=`${selected.length} data source${selected.length===1?'':'s'} selected`;detail='The selected sources are installed and ready for annotation.'}else{tone='partial';title='Core annotation only';detail='No supplementary data sources are selected.'}host.className=`wizard-readiness ${tone}`;host.querySelector('strong').textContent=title;host.querySelector('p').textContent=detail;host.querySelector('button').classList.toggle('hidden',tone==='ready')}
function selectedVcfProblem(file){if(!file)return null;if(file.error)return`This file could not be read: ${file.error}`;if(file.assembly==='GRCh37')return'GRCh37, b37, and hg19 inputs are not supported in this release. Select a GRCh38 VCF.';if(file.assembly&&file.assembly!=='GRCh38')return`AnnoCAT supports GRCh38 inputs only; this file declares ${file.assembly}.`;return null}
function selectedVcfBlockingProblem(){return selectedVcfSummaries.map((file,index)=>({file,index,problem:selectedVcfProblem(file)})).find(item=>item.problem)||null}
function setStep(step){currentStep=step;document.querySelectorAll('.wizard-panel').forEach(panel=>panel.classList.toggle('active-panel',Number(panel.dataset.step)===step));document.querySelectorAll('.steps li').forEach((item,index)=>{item.classList.toggle('current',index+1===step);item.classList.toggle('complete',index+1<step)});$('#back-step').classList.toggle('hidden',step===1);const button=$('#continue'),blocked=selectedVcfBlockingProblem();button.disabled=(step===1&&(!selectedPaths.length||Boolean(blocked)))||(step===3&&!$('#output-folder').value.trim())||step===4;button.innerHTML=step===3?'Review plan <span>→</span>':step===4?'Checking resources…':'Continue <span>→</span>';if(step===2)updateWizardReadiness();if(step===4)renderReview()}
function renderSelectedPaths(){const container=$('#selected-files'),recovery=$('#recovery-selection'),picker=$('#choose-vcfs');picker.querySelector('strong').textContent=recoveryFiles?'Choose original input VCF':'Choose VCF files';picker.querySelector('small').textContent=recoveryFiles?'Select the VCF, VCF.GZ, or BGZ that produced this output':'Select one or more VCF, VCF.GZ, or BGZ files';if(recoveryFiles){const input=recoveryFiles.input?` Original input: ${escapeHtml(fileName(recoveryFiles.input))}.`:' Now choose the original input VCF.';recovery.innerHTML=`<span><svg class="ui-icon"><use href="#icon-info"/></svg></span><p><strong>Recovering ${escapeHtml(fileName(recoveryFiles.partialVcf))}</strong><br>Complete records will be retained.${input} Select the same profile and sources used by the interrupted run.</p>`;recovery.classList.remove('hidden')}else{recovery.classList.add('hidden');recovery.innerHTML=''}if(!selectedPaths.length){container.classList.add('hidden');setStep(1);return}const blocked=selectedVcfBlockingProblem(),problem=blocked?`<div class="batch-problem" role="alert"><strong>Choose a supported input</strong><span>${escapeHtml(blocked.file?.name||fileName(selectedPaths[blocked.index]))}: ${escapeHtml(blocked.problem)}</span></div>`:'';container.innerHTML=`<div class="batch-heading"><strong>${recoveryFiles?'Original input':`${selectedPaths.length} VCF${selectedPaths.length===1?'':'s'} selected${selectedPaths.length>1?' · sequential order':''}`}</strong><button id="clear-vcfs" type="button">Clear</button></div>${problem}${selectedPaths.map((path,index)=>{const file=selectedVcfSummaries[index],fileProblem=selectedVcfProblem(file);return`<div class="batch-file${fileProblem?' invalid':''}"><span>${index+1}</span><div><strong>${escapeHtml(fileName(path))}</strong><small>${escapeHtml(path)}</small>${fileProblem?`<em>${escapeHtml(fileProblem)}</em>`:''}</div><button type="button" data-remove="${index}" aria-label="Remove ${escapeHtml(fileName(path))}">×</button></div>`}).join('')}`;container.classList.remove('hidden');$('#clear-vcfs').addEventListener('click',()=>{selectedPaths=[];selectedVcfSummaries=[];recoveryFiles=null;renderSelectedPaths()});container.querySelectorAll('[data-remove]').forEach(button=>button.addEventListener('click',()=>{if(recoveryFiles){selectedPaths=[];selectedVcfSummaries=[];delete recoveryFiles.input}else{const index=Number(button.dataset.remove);selectedPaths.splice(index,1);selectedVcfSummaries.splice(index,1)}renderSelectedPaths()}));setStep(1)}
async function chooseVcfs(){const recovering=Boolean(recoveryFiles),endpoint=recovering?'/api/pick-recovery-input':'/api/pick-vcfs',response=await fetch(endpoint,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not choose VCF files');const paths=recovering?(result.path?[result.path]:[]):result.paths;if(paths?.length){if(recovering){recoveryFiles.input=paths[0];selectedPaths=[paths[0]];selectedVcfSummaries=result.file?[result.file]:[]}else{selectedPaths=paths;selectedVcfSummaries=result.files||[]}renderSelectedPaths()}}
async function chooseRecoveryFiles(){const response=await fetch('/api/pick-recovery-files',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not choose interrupted annotation files');if(result.partialVcf){recoveryFiles=result;selectedPaths=[];selectedVcfSummaries=[];renderSelectedPaths()}}
function resourceSize(id,state=resourceStates[id],formatter=formatDataSize){if(id==='grch38-reference'&&state===resourceStates[id])return coreAnnotationSize(undefined,formatter);const item=resourcePlan.resources.find(resource=>resource.id===id),network=item?.downloadBytes?`${formatter(item.downloadBytes)} network`:item?.state==='catalog-pending'?'Network size pending':'Network size unknown',prepared=Number(state?.prepare?.preparedBytes||0);if(prepared>0)return`${network} · ${formatter(prepared)} cache on disk${state?.ready?'':' so far'}`;return item?.installMode==='stream'?`${network} · cache size measured during install`:network}
const coreResourceIds=new Set(['grch38-reference','ensembl-gff3']);
function coreAnnotationSize(items=resourcePlan.resources.filter(item=>coreResourceIds.has(item.id)),formatter=formatDataSize){const networkBytes=items.reduce((sum,item)=>sum+Number(item.downloadBytes||0),0),preparedBytes=items.reduce((sum,item)=>sum+Number(resourceStates[item.id]?.prepare?.preparedBytes||0),0),network=networkBytes?`${formatter(networkBytes)} network`:'Network size unknown';return preparedBytes?`${network} · ${formatter(preparedBytes)} cache on disk`:`${network} · cache size measured during install`}
function renderReview(){const ids=enabledSourceIds(),keepVcf=$('#keep-annotated-vcf').checked;$('#review-summary').innerHTML=`<div><span>Input</span><strong>${recoveryFiles?'Interrupted annotation recovery':`${selectedPaths.length} VCF${selectedPaths.length===1?'':'s'}`}</strong></div><div><span>Run order</span><strong>${recoveryFiles?'Resume remaining records':selectedPaths.length===1?'Single run':'Sequential separate runs'}</strong></div><div><span>Profile</span><strong>${escapeHtml($('#profile').selectedOptions[0].text)}</strong></div><div><span>Output</span><strong>${escapeHtml($('#output-folder').value||'Not selected')}</strong></div><div><span>Results</span><strong>Canonical viewer result${keepVcf?' + annotated VCF':''}</strong></div><div class="review-summary-empty" aria-hidden="true"></div>`;const required=`<div data-resource-review="core"><span class="readiness-dot"></span><strong>Core annotation data</strong><small>${escapeHtml(coreAnnotationSize())}</small><em data-reference-state>Checking…</em></div>`;$('#resource-review').innerHTML=required+ids.map(id=>`<div data-resource-review="${escapeHtml(id)}"><span class="readiness-dot"></span><strong>${escapeHtml(resourceTitle(id))}</strong><small>${escapeHtml(resourceSize(id))}</small><em data-resource-state="${escapeHtml(id)}">Checking…</em></div>`).join('');updateReviewResourceStates();refreshAppStatus().catch(console.error)}
function updateReviewResourceStates(){const core=$('[data-resource-review="core"]');if(core)core.classList.toggle('ready',lastSetupReady);enabledSourceIds().forEach(id=>document.querySelector(`[data-resource-review="${id}"]`)?.classList.toggle('ready',Boolean(resourceStates[id]?.ready)));updateReviewReadiness()}
function updateReviewReadiness(){const host=$('#review-readiness');if(!host)return;const ids=enabledSourceIds(),selectedReady=ids.every(id=>resourceStates[id]?.ready),ready=lastSetupReady&&selectedReady;host.className=`wizard-readiness ${ready?'ready':'blocked'}`;host.querySelector('strong').textContent=ready?'Ready to annotate':'Annotation resources are not ready';host.querySelector('p').textContent=ready?`${ids.length?`${ids.length} supplementary source${ids.length===1?'':'s'} plus core annotation data`:'Core annotation data'} will be used.`:'Install the missing core or selected resources before starting.';host.querySelector('button').classList.toggle('hidden',ready)}
async function refreshAnnotationStatus(snapshot){let body=snapshot;if(!body){const response=await fetch('/api/annotations/status');body=await response.json();if(!response.ok)throw new Error(body.error||'Annotation status unavailable')}const previous=lastAnnotationState.state;lastAnnotationState=body;if(previous==='running'&&body.state==='completed'){await refreshCompletedRuns();showPage('browse')}return body}
async function refreshTasks(snapshot){let body=snapshot;if(!body){const response=await fetch('/api/tasks');body=await response.json();if(!response.ok)throw new Error(body.error||'Tasks unavailable')}lastTaskSnapshots=body.tasks||body||[];renderJobs();return lastTaskSnapshots}
async function cancelAnnotation(){await fetch('/api/annotations/cancel',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}});await refreshAnnotationStatus()}
async function handleAnnotationTaskAction(runId,action,button){if(action==='cancel'&&!await confirmDestructiveAction({title:'Stop this annotation?',message:'The annotation will stop and its incomplete output will be discarded. Completed annotations and installed data sources are not affected.',confirmLabel:'Stop & discard',cancelLabel:'Keep running'}))return;const original=button.textContent;button.disabled=true;button.textContent=action==='resume'?'Resuming…':action==='pause'?'Pausing…':'Stopping…';try{if(action==='resume'){const response=await fetch('/api/annotations/resume',{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify({runId})}),body=await response.json();if(!response.ok)throw new Error(body.error||'Annotation could not resume')}else if(action==='pause'){const response=await fetch('/api/annotations/pause',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();if(!response.ok||!body.pauseRequested)throw new Error(body.error||'Annotation is no longer running')}else if(action==='cancel'){await cancelAnnotation()}await refreshAppStatus()}catch(error){showResourceNotice(error.message)}finally{button.disabled=false;button.textContent=original}}
async function startAnnotation(){const button=$('#continue'),recovering=Boolean(recoveryFiles),sourceIds=enabledSourceIds(),includeAnnotatedVcf=$('#keep-annotated-vcf').checked,blocked=selectedVcfBlockingProblem(),unknownBuild=selectedVcfSummaries.length!==selectedPaths.length||selectedVcfSummaries.some(file=>!file.assembly);if(blocked){setStep(1);return}let confirmGrch38=false;if(unknownBuild){confirmGrch38=await confirmDestructiveAction({title:'Confirm genome build',message:'The VCF header does not identify its genome build. Continue only if this file uses GRCh38 coordinates and reference alleles. AnnoCAT will validate sequence alleles against its installed GRCh38 reference.',confirmLabel:'This is GRCh38',cancelLabel:'Go back'});if(!confirmGrch38)return}const endpoint=recovering?'/api/annotations/recover':'/api/annotations/start',payload=recovering?{input:recoveryFiles.input,partialVcf:recoveryFiles.partialVcf,structuredOutput:recoveryFiles.structuredOutput,outputDirectory:$('#output-folder').value.trim(),sourceIds,includeAnnotatedVcf,confirmGrch38}:{inputs:selectedPaths,outputDirectory:$('#output-folder').value.trim(),sourceIds,includeAnnotatedVcf,confirmGrch38};button.disabled=true;button.textContent=recovering?'Starting verification…':'Starting…';try{const response=await fetch(endpoint,{method:'POST',headers:{'Content-Type':'application/json','X-AnnoCat-CSRF':'1'},body:JSON.stringify(payload)}),body=await response.json();if(!response.ok)throw new Error(body.error||(recovering?'Recovery could not start':'Annotation could not start'));clearGlobalStatusNotice();await refreshAnnotationStatus();showPage('logs')}catch(error){setAnnotationStartError(error.message)}finally{await refreshAppStatus()}}
async function chooseFolder(){const button=$('#browse-output'),message=$('#folder-message');button.disabled=true;button.textContent='Opening…';message.classList.remove('error');try{const response=await fetch('/api/pick-folder',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Native folder picker failed');if(result.path){$('#output-folder').value=result.path;message.textContent=`Selected ${result.path}`;setStep(3)}}catch(error){message.textContent=`Could not open the folder picker: ${error.message}. Start AnnoCAT with “annocat launch” from your PowerShell window.`;message.classList.add('error')}finally{button.disabled=false;button.textContent='Browse…'}}
async function refreshPaths(){portablePaths=await fetch('/api/paths').then(response=>response.json());document.querySelectorAll('#wizard-resource-path,#settings-resource-path').forEach(element=>element.textContent=portablePaths.resourceDirectory||'Unavailable');$('#settings-downloads-path').textContent=portablePaths.downloads||'Unavailable';$('#settings-results-path').textContent=portablePaths.runs||'Unavailable';return portablePaths}
async function chooseResourceFolder(event){const button=event.currentTarget,original=button.textContent;button.disabled=true;button.textContent='Opening…';try{const response=await fetch('/api/pick-resource-folder',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not change resource directory');if(result.path){await refreshPaths();await refreshAppStatus()}}catch(error){showResourceNotice(error.message)}finally{button.disabled=false;button.textContent=original}}
async function chooseResultsFolder(event){const button=event.currentTarget,original=button.textContent;button.disabled=true;button.textContent='Opening…';try{const response=await fetch('/api/pick-results-folder',{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not change results directory');if(result.path){await refreshPaths();$('#output-folder').value=result.path;await refreshCompletedRuns()}}catch(error){$('#settings-results-path').textContent=`Could not change folder: ${error.message}`}finally{button.disabled=false;button.textContent=original}}
function managedResourceIds(){return[...new Set(resourcePlan.resources.filter(resource=>resource.state==='missing').map(resource=>resource.id))]}
function resourceTaskHtml(task){const view=taskJobView(task);if(view.kind==='completed')return'';const total=Number(task.totalBytes||0),completed=Number(task.completedBytes||0),percent=Math.max(0,Math.min(100,Number(task.percent)||0)),indeterminate=task.state==='running'&&total<=0&&percent<=0,bytes=total>0?`Downloaded ${formatDataSize(completed)} of ${formatDataSize(total)}`:completed>0?`Downloaded ${formatDataSize(completed)}`:task.phase==='building-cache'?'Building cache':'Waiting for size',speed=Number(task.throughputBytesPerSecond)>0?`${formatDataSize(task.throughputBytesPerSecond)}/s`:'',chromosome=task.chromosome&&Number(task.totalChromosomes)>0?`Chromosome ${task.chromosome} of ${task.totalChromosomes} · ${task.completedChromosomes||0} ready`:Number(task.totalChromosomes)>0?`${task.completedChromosomes||0} of ${task.totalChromosomes} chromosomes ready`:'',attention=task.error||['failed','paused','cancelled','cancelling'].includes(task.state)||['reconnecting','retrying'].includes(task.phase),controls=taskActionButtons(task,(task.availableActions||[]).filter(action=>action!=='remove'));return`<article class="download-job" data-download-job="${escapeHtml(task.resourceId)}"><div class="download-job-head"><div><strong>${escapeHtml(task.title)}</strong><small>${escapeHtml(view.state)}</small></div><div class="download-job-actions">${controls}</div></div>${chromosome?`<div class="download-stage">${escapeHtml(chromosome)}</div>`:''}<div class="download-progress-meta"><span>${escapeHtml(bytes)}</span>${total>0?`<strong>${percent.toFixed(1)}%</strong>`:''}</div><div class="progress-track"${indeterminate?' aria-label="Working"':''}><div class="progress-fill${indeterminate?' indeterminate':''}" style="width:${indeterminate?35:percent}%"></div></div><div class="download-detail"><span>${attention?escapeHtml(view.detail):''}</span>${speed?`<strong class="download-speed">${escapeHtml(view.state)} · ${escapeHtml(speed)}</strong>`:''}</div></article>`}
function applyResourceStatus(id,{download,prepare}){const preparing=prepare.state==='running',prepareQueued=prepare.state==='queued',downloading=download.state==='running',validating=download.state==='validating',queued=download.state==='queued'||prepareQueued,cancelling=download.state==='cancelling',ready=prepare.state==='ready',hasArchive=download.state==='downloaded',failed=download.state==='failed'||prepare.state==='failed',hasPartial=download.downloadedBytes>0&&!hasArchive,hasPreparedPartial=Number(prepare.completedChromosomes||0)>0&&!ready,paused=['paused','cancelled'].includes(download.state)||['paused','cancelled'].includes(prepare.state)||hasPartial||(prepare.state==='idle'&&hasPreparedPartial),hasManagedData=ready||hasArchive||hasPartial||hasPreparedPartial||!['idle','missing'].includes(prepare.state),label=ready?'Installed':cancelling?'Stopping and discarding':preparing?'Installing':validating?'Verifying':failed?'Needs attention':hasArchive&&!paused?'Ready to install':downloading?'Downloading':queued?'Queued':paused?'Paused':'Not installed',busy=preparing||downloading||validating||queued||cancelling,result={download,prepare,ready,label};resourceStates[id]=result;document.querySelectorAll(`[data-resource-state="${id}"]${id==='dbnsfp'?', [data-dbnsfp-state]':''}`).forEach(node=>node.textContent=label);document.querySelectorAll(`[data-resource-review="${id}"]`).forEach(row=>row.classList.toggle('ready',ready));document.querySelectorAll(`[data-resource-storage="${id}"]`).forEach(node=>node.textContent=resourceSize(id,result));document.querySelectorAll(`[data-install="${id}"]`).forEach(button=>{button.disabled=busy;button.textContent=paused?'Resume':'Install';button.classList.toggle('resume',paused);button.classList.toggle('install',!paused);button.classList.toggle('hidden',busy||ready)});document.querySelectorAll(`[data-update="${id}"]`).forEach(button=>button.classList.toggle('hidden',!ready||busy));document.querySelectorAll(`[data-delete="${id}"]`).forEach(button=>{button.classList.toggle('hidden',!hasManagedData);button.disabled=busy});return result}
function resourceInstallationBusy(id){const status=resourceStates[id];return['running','validating','queued','cancelling'].includes(status?.download?.state)||['running','queued','cancelling'].includes(status?.prepare?.state)}
async function refreshResourceStatus(id){const[download,prepare]=await Promise.all([fetch(`/api/resources/${id}/download/status`).then(r=>r.json()),fetch(`/api/resources/${id}/prepare/status`).then(r=>r.json())]);return applyResourceStatus(id,{download,prepare})}
async function refreshDownloadStatus(providedSnapshot){if(refreshingResources)return resourceStates;refreshingResources=true;try{normalizeSourceCatalogControls();const ids=managedResourceIds();ids.forEach(ensureDeleteControl);const snapshot=providedSnapshot||await fetch('/api/resources/status').then(async response=>{const body=await response.json();if(!response.ok)throw new Error(body.error||'Resource status unavailable');return body}),entries=ids.map(id=>[id,applyResourceStatus(id,snapshot.resources[id])]),states=Object.fromEntries(entries),setup=snapshot.setup,transcripts=states['ensembl-gff3'];entries.forEach(([id,state])=>applyWizardResourceState(id,state));const coreLabel=setup.ready?'Installed':!setup.referenceReady?'Not installed':transcripts?.prepare.state==='running'?'Installing':transcripts?.download.state==='running'||transcripts?.download.state==='queued'?'Downloading':'Ensembl transcript cache not installed',coreButton=document.querySelector('[data-core-install]'),nextCoreId=setup.referenceReady?'ensembl-gff3':'grch38-reference',nextCore=states[nextCoreId];document.querySelectorAll('[data-reference-state]').forEach(node=>node.textContent=coreLabel);document.querySelector('.required-resource')?.classList.toggle('source-unavailable',!setup.ready);document.querySelectorAll('[data-update="ensembl-gff3"]').forEach(button=>button.classList.toggle('hidden',!setup.ready));if(coreButton){coreButton.classList.toggle('hidden',setup.ready);coreButton.disabled=Boolean(nextCore&&(nextCore.download.state==='running'||nextCore.download.state==='queued'||nextCore.download.state==='cancelling'||nextCore.prepare.state==='running'));coreButton.textContent=setup.referenceReady?'Install transcripts':'Install'}lastSetupReady=Boolean(setup.ready);document.querySelectorAll('[data-resource-review="core"]').forEach(row=>row.classList.toggle('ready',lastSetupReady));if(currentStep===4){const selectedReady=enabledSourceIds().every(id=>resourceStates[id]?.ready),ready=setup.ready&&selectedReady;$('#continue').disabled=!ready;$('#continue').innerHTML=ready?`${recoveryFiles?'Verify and recover':'Start annotation'} <span>→</span>`:'Install required resources'}updateWizardReadiness();updateReviewResourceStates();updateSetupModal(lastSetupReady);return states}finally{refreshingResources=false}}
async function refreshAppStatus(){const response=await fetch('/api/status'),snapshot=await response.json();if(!response.ok)throw new Error(snapshot.error||'Application status unavailable');await refreshDownloadStatus(snapshot.resources);await refreshAnnotationStatus(snapshot.annotation);await refreshTasks(snapshot.tasks);return snapshot}
async function deleteResource(id){const core=id==='grch38-reference',confirmed=await confirmDestructiveAction({title:`Remove ${resourceTitle(id)} data?`,message:`Downloaded parts and installed data for this source will be deleted.${core?' Local annotation will be unavailable until the shared core package is installed again.':''}`,confirmLabel:'Remove data',cancelLabel:'Keep data'});if(!confirmed)return;const response=await fetch(`/api/resources/${id}/delete`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)showResourceNotice(result.error||'Could not remove resource');await refreshAppStatus()}
function applyWizardResourceState(id,state){document.querySelectorAll(`#wizard-sources input[data-source="${id}"]`).forEach(input=>{input.disabled=!state.ready;if(!state.ready)input.checked=false;else if(selectedProfile()?.sourceIds.includes(id))input.checked=true;input.closest('.source-option')?.classList.toggle('source-unavailable',!state.ready)})}
function normalizeSourceCatalogControls(){if($('#source-list').dataset.normalized)return;const required=`<article data-source-card="grch38-reference"><div class="source-card-copy"><h2>Core annotation data <small>Required</small></h2><p class="source-card-description">GRCh38 reference and matching transcript cache</p><p class="source-card-storage"><strong class="resource-storage" data-resource-storage="grch38-reference">${resourceSize('grch38-reference')}</strong></p></div><div class="source-card-meta"><span class="source-state" data-reference-state>Not installed</span><div class="source-actions"><button type="button" class="hidden" data-update="ensembl-gff3">Check for updates</button><button type="button" class="danger-button hidden" data-delete="grch38-reference">Remove data</button><button class="install" data-core-install>Install</button></div></div></article>`,catalog=orderedCatalogSources().map(source=>{const catalogItem=resourcePlan.resources.find(resource=>resource.id===source.id),managed=catalogItem?.state==='missing',pending=sourceAvailabilityLabel(source),update=`<button type="button" class="hidden" data-update="${escapeHtml(source.id)}">Check for updates</button>`,configure=source.id==='dbnsfp'?'<button type="button" data-dbnsfp-config>Choose fields</button>':configurableSupplementarySources.has(source.id)?`<button type="button" data-source-fields-config="${escapeHtml(source.id)}">Choose fields</button>`:'';return`<article data-source-card="${escapeHtml(source.id)}"><div class="source-card-copy"><h2>${escapeHtml(source.name)}</h2><p class="source-card-description">${escapeHtml(source.purpose)}</p><p class="source-card-storage"><strong class="resource-storage" data-resource-storage="${escapeHtml(source.id)}">${resourceSize(source.id)}</strong>${sourceLicenseNote(source)}</p></div><div class="source-card-meta"><span class="source-state" data-resource-state="${escapeHtml(source.id)}">${managed?'Not installed':pending}</span>${managed?`<div class="source-actions">${update}${configure}<button class="install" data-install="${escapeHtml(source.id)}">Install</button></div>`:''}</div></article>`}).join('');$('#source-list').innerHTML=required+catalog;$('#source-list').dataset.normalized='true';groupPendingCatalogSources();$('#source-list [data-delete="grch38-reference"]').addEventListener('click',()=>deleteResource('grch38-reference'))}
function groupPendingCatalogSources(){const list=$('#source-list'),cards=[...list.querySelectorAll(':scope > article[data-source-card]')].filter(card=>card.dataset.sourceCard!=='grch38-reference'&&!card.querySelector('[data-install]'));if(!cards.length)return;const panel=document.createElement('details');panel.className='pending-sources-panel';panel.innerHTML=`<summary><span><strong>Sources coming later</strong><small>Catalogs and adapters that are not ready to install</small></span><b>${cards.length}</b></summary><div class="pending-source-list"></div>`;const container=panel.querySelector('.pending-source-list');cards.forEach(card=>container.append(card));list.append(panel)}
function ensureDeleteControl(id){const install=document.querySelector(`#source-list [data-install="${id}"]`);if(!install||document.querySelector(`#source-list [data-delete="${id}"]`))return;const button=document.createElement('button');button.type='button';button.className='danger-button hidden';button.dataset.delete=id;button.textContent='Remove data';button.addEventListener('click',()=>deleteResource(id));install.before(button)}
function installationConcurrency(){const value=Number(localStorage.getItem('annocat.installationConcurrency')||1);return Number.isInteger(value)&&value>=1&&value<=4?value:1}
function sourceInputMode(){const saved=localStorage.getItem('annocat.sourceInputMode');return saved==='pure-streaming'?'pure-streaming':'resumable'}
function setInstallationConcurrency(value){const normalized=Math.min(4,Math.max(1,Number(value)||1));localStorage.setItem('annocat.installationConcurrency',String(normalized));const settings=$('#installation-concurrency');if(settings)settings.value=String(normalized);return normalized}
function setSourceInputMode(value){const normalized=['resumable','hybrid-resumable'].includes(value)?'resumable':'pure-streaming';localStorage.setItem('annocat.sourceInputMode',normalized);const settings=$('#source-input-mode');if(settings)settings.value=normalized;return normalized}
async function requestResourceAction(id,path,update=false){const query=path==='prepare/start'?`?concurrency=${installationConcurrency()}&sourceMode=${sourceInputMode()}${update?'&update=true':''}`:'';const response=await fetch(`/api/resources/${id}/${path}${query}`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not ${path.replace('/',' ')}`);return result}
function showResourceNotice(message){let notice=$('#resource-notice');if(!notice){$('#source-list').insertAdjacentHTML('beforebegin','<div id="resource-notice" class="resource-notice hidden" role="status"></div>');notice=$('#resource-notice')}notice.textContent=message;notice.classList.remove('hidden')}
async function toggleDownloadJob(id,update=false){const status=resourceStates[id],plan=resourcePlan.resources.find(resource=>resource.id===id);if(plan?.installMode==='stream'||status?.download.state==='downloaded'&&!status.ready)await requestResourceAction(id,'prepare/start',update);else await requestResourceAction(id,'download/start')}
async function startCoreInstall(){const snapshot=await fetch('/api/resources/status').then(r=>r.json()),failures=[];for(const id of['grch38-reference','ensembl-gff3']){const status=snapshot.resources[id],ready=status.prepare.state==='ready',busy=['running','validating','queued','cancelling'].includes(status.download.state)||status.prepare.state==='running';if(ready||busy)continue;try{if(status.download.state==='downloaded')await requestResourceAction(id,'prepare/start');else await requestResourceAction(id,'download/start')}catch(error){failures.push(`${resourceTitle(id)}: ${error.message}`)}}if(failures.length)showResourceNotice(`Core annotation setup could not start: ${failures.join(' · ')}`);await refreshAppStatus()}
async function handleDownloadJobAction(id,action,button){if(action==='remove'){await deleteResource(id);return}if(action==='cancel'&&!await confirmDestructiveAction({title:`Stop ${resourceTitle(id)} installation?`,message:'The task will stop and its downloaded and partially installed data will be discarded. Other installed data sources are not affected.',confirmLabel:'Stop & discard',cancelLabel:'Keep running'}))return;button.disabled=true;if(['resume','install'].includes(action))button.classList.add('hidden');try{if(action==='pause')await requestResourceAction(id,'download/pause');else if(action==='cancel')await requestResourceAction(id,'download/cancel');else await toggleDownloadJob(id)}catch(error){showResourceNotice(error.message)}finally{button.disabled=false;await refreshAppStatus()}}
async function checkSourceUpdate(id,button){const original=button.textContent;button.disabled=true;button.textContent='Checking…';try{const response=await fetch(`/api/resources/${id}/updates/check`),result=await response.json();if(!response.ok)throw new Error(result.error||'Update check failed');if(!result.installed)showResourceNotice(`${resourceTitle(id)} is not installed. Available version: ${result.currentVersion}.`);else if(result.updateAvailable){if(confirm(`${resourceTitle(id)} ${result.currentVersion} is available.\n\nInstall it alongside ${result.installedVersions.join(', ')}?`))await toggleDownloadJob(id,true)}else showResourceNotice(`${resourceTitle(id)} is up to date (${result.currentVersion}).`)}catch(error){showResourceNotice(error.message)}finally{button.disabled=false;button.textContent=original;await refreshAppStatus()}}
function installableProfileResources(profile){const requested=new Set(['grch38-reference','ensembl-gff3',...(profile?.sourceIds||[])]);return resourcePlan.resources.filter(resource=>requested.has(resource.id)&&resource.state==='missing'&&!resourceStates[resource.id]?.ready)}
const dbnsfpCoordinateFields=new Set(['chr','pos(1-based)','ref','alt']);
async function loadDbnsfpConfiguration(){const response=await fetch('/api/resources/dbnsfp/config'),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not load dbNSFP field configuration');dbnsfpConfiguration=result;return result}
const dbnsfpFieldDetails={aaref:['Reference amino acid','Original amino acid for this protein change.'],aaalt:['Alternate amino acid','Substituted amino acid for this protein change.'],aapos:['Protein position','Amino-acid position within the protein.'],genename:['Gene symbol','Human-readable gene symbol associated with the prediction.'],Ensembl_geneid:['Ensembl gene ID','Stable Ensembl identifier for the gene.'],Ensembl_transcriptid:['Ensembl transcript ID','Transcript to which transcript-specific scores apply.'],Ensembl_proteinid:['Ensembl protein ID','Protein product associated with the transcript.'],Uniprot_acc:['UniProt accession','UniProt protein identifier.'],HGVSc_VEP:['Coding HGVS','VEP coding/transcript HGVS description.'],HGVSp_VEP:['Protein HGVS','VEP protein HGVS description.'],APPRIS:['APPRIS annotation','Principal or alternative transcript classification.'],GENCODE_basic:['GENCODE Basic','Whether the transcript belongs to the GENCODE Basic subset.'],TSL:['Transcript support level','Evidence-based Ensembl transcript support level.'],VEP_canonical:['Canonical transcript','Whether Ensembl marks this transcript as canonical.'],SIFT_score:['SIFT score','Missense tolerance score; lower values indicate a more damaging prediction.'],SIFT_pred:['SIFT prediction','Categorical tolerated or deleterious SIFT result.'],Polyphen2_HDIV_score:['PolyPhen-2 HDIV score','Missense damaging score trained for rare Mendelian disease variants.'],Polyphen2_HDIV_pred:['PolyPhen-2 HDIV prediction','Benign, possibly damaging, or probably damaging category.'],Polyphen2_HVAR_score:['PolyPhen-2 HVAR score','Missense damaging score trained on a broader disease-variant set.'],Polyphen2_HVAR_pred:['PolyPhen-2 HVAR prediction','Categorical PolyPhen-2 HVAR result.'],REVEL_score:['REVEL score','Missense ensemble score from 0 to 1; higher values indicate greater predicted pathogenicity.'],AlphaMissense_score:['AlphaMissense score','Deep-learning missense pathogenicity score.'],AlphaMissense_pred:['AlphaMissense prediction','Categorical benign, uncertain, or pathogenic prediction.'],PrimateAI_score:['PrimateAI score','Missense pathogenicity score informed by primate variation.'],PrimateAI_pred:['PrimateAI prediction','Categorical PrimateAI missense prediction.'],CADD_raw:['Raw CADD score','Unscaled CADD model score.'],CADD_phred:['CADD PHRED score','Rank-scaled deleteriousness score; higher values indicate stronger predicted impact.'],'GERP++_RS':['GERP++ rejected substitutions','Evolutionary constraint score; higher positive values indicate stronger conservation.'],phyloP100way_vertebrate:['phyloP 100-way score','Vertebrate base-level conservation score.'],Interpro_domain:['InterPro domain','Protein domain or functional site containing the amino-acid change.']};
dbnsfpFieldDetails.sift=['SIFT prediction','Legacy combined SIFT category and score; lower scores indicate a more damaging missense prediction.'];
dbnsfpFieldDetails.polyphen=['PolyPhen prediction','Legacy combined PolyPhen-2 category and score for predicted missense impact.'];
function readableFieldName(field){return field.replace(/_/g,' ').replace(/([a-z])([A-Z])/g,'$1 $2').replace(/\b[a-z]/g,letter=>letter.toUpperCase())}
function dbnsfpFieldPresentation(field){if(dbnsfpFieldDetails[field])return dbnsfpFieldDetails[field];const method=field.replace(/_(converted_)?rankscore$|_score$|_pred$/,'').replace(/_/g,' ');if(/rankscore$/.test(field))return[`${readableFieldName(method)} rank score`,`Percentile-like ranking of the ${method} result within dbNSFP; useful for comparing variants.`];if(/_score$/.test(field))return[readableFieldName(field),`Numeric prediction score reported by ${method}. Direction and recommended thresholds depend on that method.`];if(/_pred$/.test(field))return[readableFieldName(field),`Categorical prediction reported by ${method}, such as damaging or tolerated.`];return[readableFieldName(field),'Transcript-linked annotation retained from dbNSFP 4.9a.']}
function dbnsfpEditorHtml(configuration){const selected=new Set(configuration.selection.fields),groups=configuration.contract.groups.map(group=>{const payload=group.fields.filter(field=>!dbnsfpCoordinateFields.has(field)),required=Boolean(group.required),checked=payload.filter(field=>selected.has(field)).length;return`<section class="dbnsfp-field-group" data-dbnsfp-field-group><div class="dbnsfp-group-heading"><label><input type="checkbox" data-dbnsfp-group ${required||checked===payload.length?'checked':''} ${required||configuration.locked?'disabled':''}><span><strong>${escapeHtml(group.label||group.id)}</strong><small>${required?'Required for variant and transcript matching':`${checked} of ${payload.length} retained`}</small></span></label></div><div class="dbnsfp-field-list source-field-list">${payload.map(field=>{const[label,description]=dbnsfpFieldPresentation(field);return`<label title="${escapeHtml(field)}"><input type="checkbox" data-dbnsfp-field="${escapeHtml(field)}" ${required||selected.has(field)?'checked':''} ${required||configuration.locked?'disabled':''}><span class="source-field-copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(description)}</small><code>${escapeHtml(field)}</code></span></label>`}).join('')}</div></section>`}).join('');return`<div class="dbnsfp-field-editor" data-dbnsfp-editor><div class="dbnsfp-editor-head"><div><strong>dbNSFP retained fields</strong><small data-dbnsfp-field-count></small></div>${configuration.locked?'<span class="field-lock">Prepared cache</span>':'<div><button type="button" data-dbnsfp-recommended>Recommended</button></div>'}</div><p>Recommended keeps transcript-linked SIFT, PolyPhen, REVEL, AlphaMissense, PrimateAI, and GERP++ fields. Dedicated CADD, phyloP, gnomAD, ClinVar, dbSNP, and SpliceAI sources remain independently namespaced.</p>${groups}${configuration.locked?'<p class="dbnsfp-locked-note">This prepared dbNSFP cache already uses this field set. Remove the cache before changing it.</p>':''}</div>`}
function updateDbnsfpEditor(editor){const fields=[...editor.querySelectorAll('[data-dbnsfp-field]')],selected=fields.filter(field=>field.checked);editor.querySelector('[data-dbnsfp-field-count]').textContent=`${selected.length} of ${fields.length} cache fields retained`;editor.querySelectorAll('[data-dbnsfp-field-group]').forEach(group=>{const checkbox=group.querySelector('[data-dbnsfp-group]'),items=[...group.querySelectorAll('[data-dbnsfp-field]')];if(!checkbox||checkbox.disabled)return;checkbox.checked=items.every(item=>item.checked);checkbox.indeterminate=!checkbox.checked&&items.some(item=>item.checked);group.querySelector('small').textContent=`${items.filter(item=>item.checked).length} of ${items.length} retained`})}
function bindDbnsfpEditor(editor){if(!editor)return;editor.addEventListener('change',event=>{if(event.target.matches('[data-dbnsfp-group]'))editor.querySelectorAll(`[data-dbnsfp-field-group]`).forEach(group=>{if(group.contains(event.target))group.querySelectorAll('[data-dbnsfp-field]:not(:disabled)').forEach(field=>field.checked=event.target.checked)});updateDbnsfpEditor(editor)});const recommended=new Set(dbnsfpConfiguration?.contract?.recommendedFields||[]);editor.querySelector('[data-dbnsfp-recommended]')?.addEventListener('click',()=>{editor.querySelectorAll('[data-dbnsfp-field]:not(:disabled)').forEach(field=>field.checked=recommended.has(field.dataset.dbnsfpField));updateDbnsfpEditor(editor)});updateDbnsfpEditor(editor)}
function sameFieldSelection(left,right){if(left.length!==right.length)return false;const expected=new Set(right);return expected.size===right.length&&left.every(field=>expected.has(field))}
async function saveDbnsfpEditor(editor){if(!editor||dbnsfpConfiguration?.locked)return dbnsfpConfiguration;const required=dbnsfpConfiguration.contract.groups.filter(group=>group.required).flatMap(group=>group.fields).filter(field=>!dbnsfpCoordinateFields.has(field)),checked=[...editor.querySelectorAll('[data-dbnsfp-field]:checked')].map(input=>input.dataset.dbnsfpField),fields=[...new Set([...required,...checked])];if(sameFieldSelection(fields,dbnsfpConfiguration.selection.fields))return dbnsfpConfiguration;const response=await fetch('/api/resources/dbnsfp/config',{method:'POST',headers:{'X-AnnoCat-CSRF':'1','Content-Type':'application/json'},body:JSON.stringify({schemaVersion:dbnsfpConfiguration.selection.schemaVersion,contractId:dbnsfpConfiguration.selection.contractId,fields})}),result=await response.json();if(!response.ok)throw new Error(result.error||'Could not save dbNSFP field configuration');dbnsfpConfiguration=result;return result}
async function showDbnsfpFieldConfiguration(){try{const configuration=await loadDbnsfpConfiguration();let dialog=$('#dbnsfp-field-dialog');if(!dialog){document.body.insertAdjacentHTML('beforeend','<dialog id="dbnsfp-field-dialog" class="install-review dbnsfp-config-dialog"><form><p class="kicker">DBNSFP 4.9A</p><h2>Configure retained fields</h2><div data-dbnsfp-dialog-editor></div><div class="install-review-actions"><button type="button" data-dbnsfp-close>Cancel</button><button type="button" class="primary" data-dbnsfp-save>Save fields</button></div></form></dialog>');dialog=$('#dbnsfp-field-dialog');dialog.querySelector('[data-dbnsfp-close]').addEventListener('click',()=>dialog.close());dialog.querySelector('[data-dbnsfp-save]').addEventListener('click',async event=>{event.currentTarget.disabled=true;try{await saveDbnsfpEditor(dialog.querySelector('[data-dbnsfp-editor]'));dialog.close()}catch(error){showResourceNotice(error.message)}finally{event.currentTarget.disabled=false}})}dialog.querySelector('[data-dbnsfp-dialog-editor]').innerHTML=dbnsfpEditorHtml(configuration);bindDbnsfpEditor(dialog.querySelector('[data-dbnsfp-editor]'));dialog.querySelector('[data-dbnsfp-save]').classList.toggle('hidden',configuration.locked);dialog.showModal()}catch(error){showResourceNotice(error.message)}}
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
function favorFieldPresentation(path,leaf){const key=String(path||leaf).toLowerCase();if(/apc.*protein|protein.*apc/.test(key))return['Protein-effect evidence','FAVOR aggregate evidence for a possible effect on protein function. Higher aPC values indicate a more unusual annotation profile, not a clinical classification.'];if(/apc.*conserv|conserv.*apc/.test(key))return['Conservation evidence','FAVOR aggregate evidence that the affected base or region is evolutionarily constrained.'];if(/apc.*epigen|epigen.*apc/.test(key))return['Epigenetic evidence','FAVOR aggregate evidence from chromatin and epigenetic annotations. It describes regulatory activity, not pathogenicity by itself.'];if(/apc.*transcription|transcription.*factor|\btf\b.*apc/.test(key))return['Transcription-factor evidence','FAVOR aggregate evidence that the position overlaps transcription-factor binding information.'];if(/apc.*mapp|mapp.*apc/.test(key))return['Mappability evidence','FAVOR aggregate evidence about how uniquely sequencing reads can be assigned at this locus.'];if(/apc.*proxim|proxim.*apc/.test(key))return['Gene-proximity evidence','FAVOR aggregate evidence describing the position relative to nearby genes and transcripts.'];if(/apc.*(local.*nucleotide.*diversity|mutation.*density)|((local.*nucleotide.*diversity|mutation.*density).*apc)/.test(key))return['Regional variation evidence','FAVOR aggregate evidence about background diversity or mutation density around this position.'];if(/apc.*microrna|microrna.*apc/.test(key))return['microRNA evidence','FAVOR aggregate evidence that the position may intersect a microRNA target context.'];if(/apc.*genomic|genomic.*context|regional.*apc/.test(key))return['Genomic-context evidence','FAVOR aggregate evidence about the surrounding genomic region.'];if(/ccre/.test(key))return['Candidate regulatory element','ENCODE candidate cis-regulatory element annotation, such as promoter-like or enhancer-like activity.'];if(/chromhmm|chromatin.*state/.test(key))return['Chromatin state','Chromatin-state annotation inferred from combinations of epigenetic marks in one or more tissues.'];if(/remap|transcription.*factor|tfbs/.test(key))return['Transcription-factor overlap','Overlap with experimentally observed transcription-factor binding regions.'];if(/mappability|umap|bismap/.test(key))return['Mappability','How uniquely reads can map to this region. Low values can make a variant technically less reliable.'];if(/distance.*(tss|tes)|nearest.*gene|gene.*distance/.test(key))return['Distance to gene','Distance from this position to a nearby gene or transcript landmark. Proximity is context, not proof that the gene is affected.'];return[readableFieldName(leaf||'FAVOR field'),'FAVOR annotation discovered in this report. Its meaning depends on the named FAVOR field and release.']}
function evidenceFieldPresentationBase(field){const source=String(field?.sourceId||'').toLowerCase(),path=String(field?.fieldPath||''),leaf=path.split(/[.\[\]]/).filter(Boolean).pop()||path;let details;if(source.includes('dbnsfp'))details=dbnsfpFieldPresentation(leaf);else if(source.includes('favor'))details=favorFieldPresentation(path,leaf);else{const resourceId=['gnomad-genomes','clinvar','dbsnp','gnomad','phylop','cadd','spliceai','revel'].find(id=>source===id||source.startsWith(`${id}-`)||source.startsWith(`${id}@`));details=resourceId?supplementaryFieldPresentation(resourceId,leaf):[readableFieldName(leaf||'Evidence field'),'Annotation field discovered in this report.']}return{label:details[0],description:details[1],fieldPath:path,sourceId:field?.sourceId||'',valueType:field?.valueType||'unknown'}}
async function loadSupplementaryFieldConfiguration(resourceId){const response=await fetch(`/api/resources/${encodeURIComponent(resourceId)}/fields`),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not load ${resourceTitle(resourceId)} field configuration`);supplementaryFieldConfigurations.set(resourceId,result);return result}
function supplementaryFieldEditorHtml(resourceId,configuration){const selected=new Set(configuration.selection.fields),groups=configuration.contract.groups.map(group=>{const checked=group.fields.filter(field=>selected.has(field)).length,required=Boolean(group.required);return`<section class="dbnsfp-field-group" data-source-field-group><div class="dbnsfp-group-heading"><label><input type="checkbox" data-source-field-group-toggle ${required||checked===group.fields.length?'checked':''} ${required||configuration.locked?'disabled':''}><span><strong>${escapeHtml(group.label||group.id)}</strong><small>${required?'Required':`${checked} of ${group.fields.length} retained`}</small></span></label></div><div class="dbnsfp-field-list source-field-list">${group.fields.map(field=>{const[label,description]=supplementaryFieldPresentation(resourceId,field);return`<label title="${escapeHtml(field)}"><input type="checkbox" data-source-field="${escapeHtml(field)}" ${required||selected.has(field)?'checked':''} ${required||configuration.locked?'disabled':''}><span class="source-field-copy"><strong>${escapeHtml(label)}</strong><small>${escapeHtml(description)}</small><code>${escapeHtml(field)}</code></span></label>`}).join('')}</div></section>`}).join('');const fullByDefault=resourceId==='gnomad'||resourceId==='gnomad-genomes';return`<div class="dbnsfp-field-editor" data-source-field-editor="${escapeHtml(resourceId)}"><div class="dbnsfp-editor-head"><div><strong>${escapeHtml(resourceTitle(resourceId))} retained fields</strong><small data-source-field-count></small></div>${configuration.locked?'<span class="field-lock">Prepared cache</span>':`<button type="button" data-source-field-defaults>${fullByDefault?'Select all':'Restore defaults'}</button>`}</div><p>Choose what AnnoCat keeps in the local fastVEP cache. Keeping fewer fields reduces cache size. Every choice below is supported by the bundled parser and appears under this source in results.</p>${groups}${configuration.locked?`<p class="dbnsfp-locked-note">This prepared ${escapeHtml(resourceTitle(resourceId))} cache already uses this field set. Remove the cache before changing it.</p>`:''}</div>`}
function updateSupplementaryFieldEditor(editor){if(!editor)return;const checked=editor.querySelectorAll('[data-source-field]:checked').length,total=editor.querySelectorAll('[data-source-field]').length;editor.querySelector('[data-source-field-count]').textContent=`${checked} of ${total} fields retained`;editor.querySelectorAll('[data-source-field-group]').forEach(group=>{const toggle=group.querySelector('[data-source-field-group-toggle]'),fields=[...group.querySelectorAll('[data-source-field]')],selected=fields.filter(field=>field.checked).length;if(toggle&&!toggle.disabled){toggle.checked=fields.length>0&&selected===fields.length;toggle.indeterminate=selected>0&&!toggle.checked;group.querySelector('small').textContent=`${selected} of ${fields.length} retained`}})}
function bindSupplementaryFieldEditor(editor){if(!editor)return;const resourceId=editor.dataset.sourceFieldEditor,configuration=supplementaryFieldConfigurations.get(resourceId),fullByDefault=resourceId==='gnomad'||resourceId==='gnomad-genomes';editor.addEventListener('change',event=>{if(event.target.matches('[data-source-field-group-toggle]'))event.target.closest('[data-source-field-group]').querySelectorAll('[data-source-field]:not(:disabled)').forEach(field=>field.checked=event.target.checked);updateSupplementaryFieldEditor(editor)});const defaults=new Set(configuration.contract.groups.filter(group=>fullByDefault||group.default||group.required).flatMap(group=>group.fields));editor.querySelector('[data-source-field-defaults]')?.addEventListener('click',()=>{editor.querySelectorAll('[data-source-field]:not(:disabled)').forEach(field=>field.checked=defaults.has(field.dataset.sourceField));updateSupplementaryFieldEditor(editor)});updateSupplementaryFieldEditor(editor)}
async function saveSupplementaryFieldEditor(editor){if(!editor)return;const resourceId=editor.dataset.sourceFieldEditor,configuration=supplementaryFieldConfigurations.get(resourceId);if(configuration?.locked)return configuration;const fields=[...editor.querySelectorAll('[data-source-field]:checked')].map(input=>input.dataset.sourceField);if(!fields.length)throw new Error(`Select at least one ${resourceTitle(resourceId)} field`);if(sameFieldSelection(fields,configuration.selection.fields))return configuration;const response=await fetch(`/api/resources/${encodeURIComponent(resourceId)}/fields`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1','Content-Type':'application/json'},body:JSON.stringify({schemaVersion:configuration.selection.schemaVersion,contractId:configuration.selection.contractId,fields})}),result=await response.json();if(!response.ok)throw new Error(result.error||`Could not save ${resourceTitle(resourceId)} fields`);supplementaryFieldConfigurations.set(resourceId,result);return result}
async function showSupplementaryFieldConfiguration(resourceId){try{const configuration=await loadSupplementaryFieldConfiguration(resourceId);let dialog=$('#supplementary-field-dialog');if(!dialog){document.body.insertAdjacentHTML('beforeend','<dialog id="supplementary-field-dialog" class="install-review dbnsfp-config-dialog"><form><p class="kicker" data-source-field-kicker></p><h2>Configure retained fields</h2><div data-source-field-dialog-editor></div><div class="install-review-actions"><button type="button" data-source-field-close>Cancel</button><button type="button" class="primary" data-source-field-save>Save fields</button></div></form></dialog>');dialog=$('#supplementary-field-dialog');dialog.querySelector('[data-source-field-close]').addEventListener('click',()=>dialog.close());dialog.querySelector('[data-source-field-save]').addEventListener('click',async event=>{event.currentTarget.disabled=true;try{await saveSupplementaryFieldEditor(dialog.querySelector('[data-source-field-editor]'));dialog.close()}catch(error){showResourceNotice(error.message)}finally{event.currentTarget.disabled=false}})}dialog.querySelector('[data-source-field-kicker]').textContent=resourceTitle(resourceId);dialog.querySelector('[data-source-field-dialog-editor]').innerHTML=supplementaryFieldEditorHtml(resourceId,configuration);bindSupplementaryFieldEditor(dialog.querySelector('[data-source-field-editor]'));const save=dialog.querySelector('[data-source-field-save]');save.disabled=false;save.classList.toggle('hidden',configuration.locked);dialog.showModal()}catch(error){showResourceNotice(error.message)}}
function profileReviewResources(profile,installable){const items=[...installable],seen=new Set(items.map(item=>item.id));for(const id of profile.sourceIds||[]){if(seen.has(id)||!resourceStates[id]?.ready||!(id==='dbnsfp'||configurableSupplementarySources.has(id)))continue;const item=resourcePlan.resources.find(resource=>resource.id===id);if(item){items.push(item);seen.add(id)}}return items}
async function profileInstallItemsHtml(items){const sections=[],coreItems=items.filter(item=>coreResourceIds.has(item.id));if(coreItems.length)sections.push(`<div class="install-review-row"><strong>Core annotation data</strong><span>${escapeHtml(coreAnnotationSize(coreItems))}</span></div>`);for(const item of items.filter(item=>!coreResourceIds.has(item.id))){let configuration=null,editor='';if(item.id==='dbnsfp'){configuration=await loadDbnsfpConfiguration();editor=dbnsfpEditorHtml(configuration)}else if(configurableSupplementarySources.has(item.id)){configuration=await loadSupplementaryFieldConfiguration(item.id);editor=supplementaryFieldEditorHtml(item.id,configuration)}const size=resourceSize(item.id);if(!configuration){sections.push(`<div class="install-review-row"><strong>${escapeHtml(resourceTitle(item.id))}</strong><span>${escapeHtml(size)}</span></div>`);continue}const fieldCount=configuration.selection.fields.length,state=configuration.locked?'Installed':'Customize',title=resourceTitle(item.id);sections.push(`<details class="profile-install-source"><summary title="Expand ${escapeHtml(title)} retained fields"><strong>${escapeHtml(title)}</strong><span>${escapeHtml(size)}</span><small class="install-disclosure-label"><span>${fieldCount} field${fieldCount===1?'':'s'} · ${state}</span><svg class="ui-icon install-disclosure-chevron" aria-hidden="true"><use href="#icon-chevron-down"></use></svg></small></summary>${editor}</details>`)}return sections.join('')}
function updateProfileInstallRuntimeCopy(dialog){const count=installationConcurrency(),mode=sourceInputMode(),installable=Number(dialog.dataset.installableCount||0),total=Number(dialog.dataset.installNetworkBytes||0),unknownSizes=Number(dialog.dataset.installUnknownSizes||0),readyCache=Number(dialog.dataset.readyCacheBytes||0),network=unknownSizes?`${total?`${formatDataSize(total)} known · `:''}${unknownSizes} rolling size${unknownSizes===1?'':'s'} resolved at install`:`${formatDataSize(total)} network`;dialog.querySelector('[data-install-summary]').textContent=`${installable} available installation${installable===1?'':'s'} · ${network} · up to ${count} source${count===1?'':'s'} at once${readyCache?` · ${formatDataSize(readyCache)} cache already on disk`:''}`;dialog.querySelector('[data-install-stream-note]').textContent=mode==='pure-streaming'?'Streaming uses less disk, but an interruption may restart the current source part.':'Resumable saves a temporary part so interrupted downloads continue without replay.'}
function updateExpandableSummary(details){const summary=details.querySelector(':scope > summary');if(!summary)return;const subject=details.classList.contains('install-runtime-help')?'download settings':`${summary.querySelector('strong')?.textContent||'source'} retained fields`;summary.title=`${details.open?'Collapse':'Expand'} ${subject}`}
function bindExpandableSummaries(dialog){dialog.querySelectorAll('.profile-install-source,.install-runtime-help').forEach(details=>{details.ontoggle=()=>updateExpandableSummary(details);updateExpandableSummary(details)})}
async function showProfileInstallReview(profileId){
  const profile=profiles.find(item=>item.id===profileId);
  if(!profile)return;
  const installable=installableProfileResources(profile),reviewItems=profileReviewResources(profile,installable),installableIds=new Set(installable.map(item=>item.id)),pending=(profile.sourceIds||[]).filter(id=>id!=='fastvep'&&!installableIds.has(id)&&!resourceStates[id]?.ready).map(id=>sources.find(source=>source.id===id)?.name||id),total=installable.reduce((sum,item)=>sum+(item.downloadBytes||0),0),unknownSizes=installable.filter(item=>!item.downloadBytes).length,readyCache=(profile.sourceIds||[]).reduce((sum,id)=>sum+Number(resourceStates[id]?.prepare?.preparedBytes||0),0);
  let installItemsHtml='';
  try{installItemsHtml=await profileInstallItemsHtml(reviewItems)}catch(error){showResourceNotice(error.message);return}
  let dialog=$('#profile-install-review');
  if(!dialog){
    document.body.insertAdjacentHTML('beforeend','<dialog id="profile-install-review" class="install-review profile-install-review" tabindex="-1"><form><p class="kicker">RECOMMENDED PROFILE</p><h2 data-install-title></h2><p data-install-summary></p><div data-install-items></div><p class="install-pending" data-install-pending></p><div class="install-runtime-footer"><details class="install-runtime-help" open><summary><strong>Download settings</strong><small class="install-disclosure-label"><span>Options</span><svg class="ui-icon install-disclosure-chevron" aria-hidden="true"><use href="#icon-chevron-down"></use></svg></small></summary><div class="install-runtime-options"><label><span>Download safety</span><select data-install-source-mode><option value="resumable">Resumable — Recommended</option><option value="pure-streaming">Pure streaming — Uses less temporary disk</option></select></label><label><span>Concurrent installs</span><select data-install-concurrency><option value="1">1 — Recommended</option><option value="2">2 — Faster</option><option value="3">3 — High resource use</option><option value="4">4 — Maximum resource use</option></select></label></div><p class="install-stream-note" data-install-stream-note></p></details><div class="install-review-actions"><button type="button" data-close-profile-install>Cancel</button><button type="button" class="primary" data-confirm-profile-install>Start installation</button></div></div></form></dialog>');
    dialog=$('#profile-install-review');
    dialog.querySelector('[data-close-profile-install]').addEventListener('click',()=>dialog.close());
    dialog.querySelector('[data-confirm-profile-install]').addEventListener('click',event=>queueProfileInstall(event.currentTarget.dataset.confirmProfileInstall,dialog));
    dialog.querySelector('[data-install-source-mode]').addEventListener('change',event=>{setSourceInputMode(event.target.value);updateProfileInstallRuntimeCopy(dialog)});
    dialog.querySelector('[data-install-concurrency]').addEventListener('change',event=>{setInstallationConcurrency(event.target.value);updateProfileInstallRuntimeCopy(dialog)});
  }
  dialog.dataset.installableCount=String(installable.length);
  dialog.dataset.installNetworkBytes=String(total);
  dialog.dataset.installUnknownSizes=String(unknownSizes);
  dialog.dataset.readyCacheBytes=String(readyCache);
  dialog.querySelector('[data-install-title]').textContent=`Install ${profile.name} recommendations`;
  dialog.querySelector('[data-install-source-mode]').value=sourceInputMode();
  dialog.querySelector('[data-install-concurrency]').value=String(installationConcurrency());
  updateProfileInstallRuntimeCopy(dialog);
  dialog.querySelector('[data-install-items]').innerHTML=installItemsHtml;
  bindDbnsfpEditor(dialog.querySelector('[data-dbnsfp-editor]'));
  dialog.querySelectorAll('[data-source-field-editor]').forEach(bindSupplementaryFieldEditor);
  const downloadSettings=dialog.querySelector('.install-runtime-help');
  downloadSettings.open=true;
  bindExpandableSummaries(dialog);
  const pendingMessage=dialog.querySelector('[data-install-pending]');
  pendingMessage.textContent=pending.length?`${pending.join(', ')} are still pending verified installers or catalogs and will not be started.`:'';
  pendingMessage.classList.toggle('hidden',pending.length===0);
  const confirmButton=dialog.querySelector('[data-confirm-profile-install]');
  confirmButton.dataset.confirmProfileInstall=profileId;
  confirmButton.disabled=installable.length===0;
  dialog.showModal();
  dialog.focus({preventScroll:true});
}
async function queueProfileInstall(profileId,dialog){const profile=profiles.find(item=>item.id===profileId),resources=installableProfileResources(profile),failures=[],button=dialog?.querySelector('[data-confirm-profile-install]');if(button)button.disabled=true;try{if(!resourceInstallationBusy('dbnsfp'))await saveDbnsfpEditor(dialog?.querySelector('[data-dbnsfp-editor]'));for(const editor of dialog?.querySelectorAll('[data-source-field-editor]')||[]){const resourceId=editor.dataset.sourceFieldEditor;if(!resourceInstallationBusy(resourceId))await saveSupplementaryFieldEditor(editor)}for(const resource of resources.filter(item=>item.installMode!=='stream')){const status=await refreshResourceStatus(resource.id);if(status.ready||resourceInstallationBusy(resource.id))continue;try{if(status.download.state==='downloaded')await requestResourceAction(resource.id,'prepare/start');else await requestResourceAction(resource.id,'download/start')}catch(error){failures.push(`${resourceTitle(resource.id)}: ${error.message}`)}}if(resources.some(item=>item.installMode==='stream')){try{const response=await fetch(`/api/profiles/${profileId}/prepare/start?concurrency=${installationConcurrency()}&sourceMode=${sourceInputMode()}`,{method:'POST',headers:{'X-AnnoCat-CSRF':'1'}}),body=await response.json();if(!response.ok)throw new Error(body.error||'Profile installation could not start')}catch(error){failures.push(error.message)}}if(!failures.length)dialog?.close()}catch(error){failures.push(error.message)}finally{if(button)button.disabled=false}await refreshAppStatus();if(failures.length)showResourceNotice(`Some installations could not start: ${failures.join(' · ')}`)}
async function start(){
  [variants,sources,profiles,resourcePlan,evidenceCalibrations,portablePaths]=await Promise.all([fetch('/api/demo/variants').then(r=>r.json()),fetch('/api/sources').then(r=>r.json()),fetch('/api/profiles').then(r=>r.json()),fetch('/api/resources/plan').then(r=>r.json()),fetch('/api/evidence-calibrations').then(r=>r.json()),fetch('/api/paths').then(r=>r.json())]);
  if(portablePaths.runs){$('#output-folder').value=portablePaths.runs;$('#folder-message').textContent='Default results directory. Use Browse to change this run only.'}
  $('#settings-resource-path').textContent=portablePaths.resourceDirectory||'Unavailable';
  $('#settings-downloads-path').textContent=portablePaths.downloads||'Unavailable';
  $('#settings-results-path').textContent=portablePaths.runs||'Unavailable';
  renderColumns();renderTable();renderProfiles();renderWizardSources();normalizeSourceCatalogControls();
  document.addEventListener('click',event=>{const install=event.target.closest('[data-install]'),core=event.target.closest('[data-core-install]'),jobButton=event.target.closest('[data-job-action]'),jobCard=jobButton?.closest('[data-download-job]'),annotationCard=jobButton?.closest('[data-annotation-task]');if(install)handleDownloadJobAction(install.dataset.install,'resume',install);if(core)startCoreInstall();if(jobButton&&jobCard)handleDownloadJobAction(jobCard.dataset.downloadJob,jobButton.dataset.jobAction,jobButton);if(jobButton&&annotationCard)handleAnnotationTaskAction(annotationCard.dataset.annotationTask,jobButton.dataset.jobAction,jobButton);const page=event.target.closest('[data-page-link]')?.dataset.pageLink;if(page){setupDismissed=true;showPage(page)}});
  document.querySelectorAll('[data-pick-resource]').forEach(button=>button.addEventListener('click',chooseResourceFolder));
  document.querySelectorAll('[data-pick-results-folder]').forEach(button=>button.addEventListener('click',chooseResultsFolder));
  const savedProfile=localStorage.getItem('annocat.defaultProfile')||'wgs',showSetup=localStorage.getItem('annocat.showSetup')!=='false';
  localStorage.removeItem('annocat.resultDensity');
  document.body.classList.remove('compact-results');
  $('#default-profile').value=savedProfile;$('#show-setup').checked=showSetup;
  const profileOption=[...$('#profile').options].find(option=>option.value===savedProfile);if(profileOption){$('#profile').value=savedProfile;renderWizardSources()}
  setupDismissed=!showSetup;
  $('#show-setup').addEventListener('change',event=>{localStorage.setItem('annocat.showSetup',event.target.checked);setupDismissed=!event.target.checked;if(setupDismissed)$('#first-run').classList.remove('visible');else updateSetupModal(lastSetupReady)});
  $('#default-profile').addEventListener('change',event=>localStorage.setItem('annocat.defaultProfile',event.target.value));
  $('#reset-preferences').addEventListener('click',()=>{localStorage.removeItem('annocat.showSetup');localStorage.removeItem('annocat.defaultProfile');localStorage.removeItem('annocat.installationConcurrency');localStorage.removeItem('annocat.sourceInputMode');$('#show-setup').checked=true;$('#default-profile').value='wgs';installationConcurrencySelect.value='1';sourceInputModeSelect.value='resumable'});
  await Promise.all([refreshAppStatus(),refreshCompletedRuns()]);
  setInterval(()=>refreshAppStatus().catch(console.error),1000);setInterval(()=>{if($('#browse').classList.contains('active-page'))refreshCompletedRuns().catch(console.error)},5000)
}
document.querySelectorAll('.nav-item').forEach(button=>button.addEventListener('click',()=>{showPage(button.dataset.page);if(button.dataset.page==='browse')refreshCompletedRuns().catch(console.error)}));$('#open-demo').addEventListener('click',async()=>{variants=await fetch('/api/demo/variants').then(response=>response.json());renderTable();document.querySelector('#results .results-heading h1').textContent='Synthetic demonstration';document.querySelector('#results .results-heading p').textContent='No personal variant files are loaded.';showPage('results')});$('#back-to-browse').addEventListener('click',()=>{showPage('browse');refreshCompletedRuns().catch(console.error)});$('#choose-vcfs').addEventListener('click',()=>chooseVcfs().catch(error=>showResourceNotice(error.message)));$('#recover-annotation').addEventListener('click',()=>chooseRecoveryFiles().catch(error=>showResourceNotice(error.message)));$('#vcf-files').addEventListener('change',event=>{recoveryFiles=null;selectedPaths=[...event.target.files].map(file=>file.name);renderSelectedPaths()});$('#profile').addEventListener('change',applyProfile);$('#browse-output').addEventListener('click',chooseFolder);$('#output-directory-fallback').addEventListener('change',event=>{const file=event.target.files[0];if(file){const folder=file.webkitRelativePath.split('/')[0];$('#output-folder').value=folder;$('#folder-message').textContent=`Selected “${folder}” using compatibility mode.`;setStep(3)}});$('#output-folder').addEventListener('input',()=>setStep(3));$('#continue').addEventListener('click',()=>{if(currentStep<4)setStep(currentStep+1);else startAnnotation()});$('#back-step').addEventListener('click',()=>setStep(currentStep-1));$('#search').addEventListener('input',scheduleResultSearch);$('#columns').addEventListener('click',event=>{event.stopPropagation();toggleResultPopover('column-menu')});start().catch(error=>console.error(error));
document.addEventListener('click',event=>{const pageButton=event.target.closest('[data-status-page]'),disableButton=event.target.closest('[data-status-disable-source]'),dismissButton=event.target.closest('[data-status-dismiss]');if(pageButton)showPage(pageButton.dataset.statusPage);if(disableButton){const sourceId=disableButton.dataset.statusDisableSource,input=document.querySelector(`#wizard-sources input[data-source="${sourceId}"]`);if(input){input.checked=false;$('#profile').value='custom';$('#wizard-sources .profile-badge').forEach(badge=>badge.remove())}clearGlobalStatusNotice();showPage('annotate');setStep(4);refreshAppStatus().catch(console.error)}if(dismissButton)clearGlobalStatusNotice()});
$('#setup-annotation').addEventListener('click',()=>{setupDismissed=true;$('#first-run').classList.add('hidden');showPage('resources')});$('#setup-open-results').addEventListener('click',openExistingResults);$('#setup-later').addEventListener('click',()=>{setupDismissed=true;$('#first-run').classList.add('hidden')});document.querySelector('#browse .choice:first-child').addEventListener('click',openExistingResults);
document.addEventListener('click',event=>{const id=event.target.dataset.update;if(id)checkSourceUpdate(id,event.target)});
const installationConcurrencySelect=$('#installation-concurrency');
installationConcurrencySelect.value=String(installationConcurrency());
installationConcurrencySelect.addEventListener('change',event=>setInstallationConcurrency(event.target.value));
const sourceInputModeSelect=$('#source-input-mode');
sourceInputModeSelect.value=sourceInputMode();
sourceInputModeSelect.addEventListener('change',event=>setSourceInputMode(event.target.value));
document.addEventListener('click',event=>{if(event.target.closest('[data-dbnsfp-config]'))showDbnsfpFieldConfiguration()});
document.addEventListener('click',event=>{const button=event.target.closest('[data-source-fields-config]');if(button)showSupplementaryFieldConfiguration(button.dataset.sourceFieldsConfig)});
document.addEventListener('click',event=>{const button=event.target.closest('[data-install],[data-core-install]');if(button){button.disabled=true;button.classList.add('hidden')}},true);
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
const resultPopovers=[['result-filters','filters'],['column-menu','columns']];
function closeResultPopovers(){resultPopovers.forEach(([panel,button])=>{$(`#${panel}`).classList.add('hidden');$(`#${button}`).setAttribute('aria-expanded','false')})}
function toggleResultPopover(panelId){
  const entry=resultPopovers.find(([panel])=>panel===panelId),panel=$(`#${panelId}`),open=panel.classList.contains('hidden');
  closeResultPopovers();
  if(!open||!entry)return;
  const button=$(`#${entry[1]}`),container=$('#results .results-panel'),containerBounds=container.getBoundingClientRect(),buttonBounds=button.getBoundingClientRect();
  panel.style.top=`${Math.round(buttonBounds.bottom-containerBounds.top-container.clientTop+6)}px`;
  panel.classList.remove('hidden');
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
  return`<div class="annotation-row population-summary-row tone-${escapeHtml(presentation.tone||'neutral')}" ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><span class="annotation-field"><strong>${escapeHtml(label)}</strong></span><b>${escapeHtml(displayDetailValue(presentation.display))}</b></div>`
}
function populationEvidenceRow(item,label,isMaximum=false,showSource=false){
  const interpreted=evidenceValuePresentation(item),tooltip=evidenceValueTooltip(item,item.value,{includeSource:true}),sourcePrefix=showSource?`${resourceTitle(item.sourceId)} · `:'';
  return`<div class="annotation-row tone-${interpreted.tone}${isMaximum?' group-maximum-row':''}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(`${sourcePrefix}${label}`)}</strong>${isMaximum?'<small>Highest</small>':''}</span><b>${escapeHtml(interpreted.display)}</b></div>`
}
function populationEvidenceSubgroup(items,preferredSourceId='',empty='None reported'){
  if(!items.length)return`<div class="key-evidence-subgroup population-evidence" data-evidence-group="population"><div class="key-evidence-subheading"><strong>Population</strong></div><div class="annotation-list"><div class="key-evidence-empty">${escapeHtml(empty)}</div></div></div>`;
  const sourceKey=value=>String(value||'').toLowerCase(),preferredKey=sourceKey(preferredSourceId),primarySourceId=items.find(item=>sourceKey(item.sourceId)===preferredKey)?.sourceId||items.find(item=>sourceKey(item.sourceId).includes('gnomad'))?.sourceId||items[0].sourceId,primaryKey=sourceKey(primarySourceId),primaryItems=items.filter(item=>sourceKey(item.sourceId)===primaryKey);
  const findKind=kind=>primaryItems.find(item=>populationFieldKind(item)===kind),overall=findKind('overall'),alleleCount=findKind('alleleCount'),alleleNumber=findKind('alleleNumber'),homozygotes=findKind('homozygotes'),reported=item=>item&&evidenceValuePresentation(item).tone!=='missing';
  const ancestryRows=primaryItems.map(item=>{const ancestry=populationAncestryEntry(item);if(!ancestry)return null;const number=Number(decodeEvidenceValue(item.value));return{...ancestry,item,number:Number.isFinite(number)&&number>=0&&number<=1?number:Number.NEGATIVE_INFINITY}}).filter(Boolean).sort((left,right)=>right.number-left.number||left.label.localeCompare(right.label));
  const explicitMaximum=findKind('groupMaximum'),maximumLabelItem=findKind('groupMaximumLabel'),derivedMaximum=ancestryRows.find(row=>Number.isFinite(row.number)&&row.number!==Number.NEGATIVE_INFINITY),maximumItem=reported(explicitMaximum)?explicitMaximum:derivedMaximum?.item,maximumPresentation=maximumItem?evidenceValuePresentation(maximumItem):{display:'Not reported',tone:'missing'},maximumCode=populationGroupCode(decodeEvidenceValue(maximumLabelItem?.value))||derivedMaximum?.code||'',maximumLabel=maximumCode?populationAncestryLabels[maximumCode]:maximumLabelItem?readableTerm(displayDetailValue(maximumLabelItem.value)):'';
  const overallPresentation=overall?evidenceValuePresentation(overall):{display:'Not reported',tone:'missing'},countPresentation=populationCountPresentation(alleleCount),numberPresentation=populationCountPresentation(alleleNumber),homozygotePresentation=populationCountPresentation(homozygotes),countReported=countPresentation.tone!=='missing',numberReported=numberPresentation.tone!=='missing',countAndNumber={display:countReported&&numberReported?`${countPresentation.display} of ${numberPresentation.display}`:countReported?`${countPresentation.display}; total not reported`:numberReported?`Alternate count not reported; ${numberPresentation.display} measured`:'Not reported',tone:countReported||numberReported?'neutral':'missing'},maximumDisplay={...maximumPresentation,display:maximumLabel&&maximumPresentation.tone!=='missing'?`${maximumPresentation.display} · ${maximumLabel}`:maximumPresentation.display};
  const sourceTitle=resourceTitle(primarySourceId),overallTooltip=overall?evidenceValueTooltip(overall,overall.value,{includeSource:true}):`No overall allele frequency was reported by ${sourceTitle}.`,maximumTooltip=maximumItem?`${evidenceValueTooltip(maximumItem,maximumItem.value,{includeSource:true})}${maximumLabel?` Highest group: ${maximumLabel}.`:''}`:`No ancestry-group maximum was reported by ${sourceTitle}.`,countTooltip=countReported||numberReported?`Alternate allele copies observed: ${countPresentation.display}. Total alleles with usable genotype calls: ${numberPresentation.display}. Source: ${sourceTitle}.`:`No allele counts were reported by ${sourceTitle}.`,homozygoteTooltip=homozygotes?evidenceValueTooltip(homozygotes,homozygotes.value,{includeSource:true}):`No alternate-homozygote count was reported by ${sourceTitle}.`;
  const primaryAdditional=primaryItems.filter(item=>!populationFieldKind(item)&&!populationAncestryEntry(item)),secondaryItems=items.filter(item=>sourceKey(item.sourceId)!==primaryKey),additionalItems=sortPopulationEvidence([...primaryAdditional,...secondaryItems]);
  const ancestryHtml=ancestryRows.map(row=>populationEvidenceRow(row.item,row.label,row.code===maximumCode)).join(''),additionalHtml=additionalItems.map(item=>populationEvidenceRow(item,evidenceFieldPresentation(item).label,false,sourceKey(item.sourceId)!==primaryKey)).join('');
  return`<div class="key-evidence-subgroup population-evidence" data-evidence-group="population"><div class="key-evidence-subheading"><strong>Population</strong></div><div class="annotation-list population-summary-list">${populationSummaryMetric('Overall allele frequency',overallPresentation,overallTooltip)}${populationSummaryMetric('Highest group AF',maximumDisplay,maximumTooltip)}${populationSummaryMetric('Alternate alleles observed',countAndNumber,countTooltip)}${populationSummaryMetric('Alternate homozygotes',homozygotePresentation,homozygoteTooltip)}</div>${ancestryRows.length?`<details class="population-breakdown collapsible-detail"><summary><strong>Genetic ancestry breakdown</strong>${detailToggleControl()}</summary><div class="annotation-list">${ancestryHtml}</div></details>`:''}${additionalItems.length?`<details class="population-breakdown collapsible-detail"><summary><strong>Additional frequency data (${additionalItems.length})</strong>${detailToggleControl()}</summary><div class="annotation-list">${additionalHtml}</div></details>`:''}</div>`
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
  return`<div class="annotation-row tone-${interpreted.tone}" data-field-path="${escapeHtml(String(item.fieldPath||'').toLowerCase())}" title="${escapeHtml(tooltip)}"><span class="annotation-field"><strong>${escapeHtml(presentation.label)}</strong></span>${renderedValue}</div>`
}
function detailToggleControl(){return`<b class="detail-toggle-label" aria-hidden="true">${prototypeIcon('chevron-right')}</b>`}
function detailEvidenceSubgroup(title,items,empty='None reported',options={}){if(options.kind==='population')return populationEvidenceSubgroup(items,options.preferredSourceId,empty);return`<div class="key-evidence-subgroup" data-evidence-group="${escapeHtml(title.toLowerCase())}"><div class="key-evidence-subheading"><strong>${escapeHtml(title)}</strong></div><div class="annotation-list">${items.length?items.map(item=>annotationRow(item)).join(''):`<div class="key-evidence-empty">${escapeHtml(empty)}</div>`}</div></div>`}
function groupedEvidenceSection(title,groups,{open=false,className='',extra='',sectionKey=''}={}){
  const expanded=sectionKey?variantDetailOpenSections.has(sectionKey):open,stateAttribute=sectionKey?` data-variant-detail-section="${escapeHtml(sectionKey)}"`:'';
  return`<section class="detail-section annotation-section evidence-domain-section ${escapeHtml(className)}"><details class="evidence-domain collapsible-detail"${stateAttribute} ${expanded?'open':''}><summary><strong>${escapeHtml(title)}</strong>${detailToggleControl()}</summary><div class="key-evidence-subgroups">${groups.map(([groupTitle,items,empty,options])=>detailEvidenceSubgroup(groupTitle,items,empty,options)).join('')}${extra}</div></details></section>`
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
  const rendered=facts.filter(Boolean).map(([label,value,tooltip='',tone='neutral'])=>`<div ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="tone-${escapeHtml(tone)}">${escapeHtml(displayDetailValue(value))}</dd></div>`).join('');
  return`<section class="transcript-fact-group"><div class="transcript-fact-heading"><strong>${escapeHtml(title)}</strong></div><dl>${rendered}</dl></section>`
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
  return`<div ${tooltip?`title="${escapeHtml(tooltip)}"`:''}><dt>${escapeHtml(label)}</dt><dd class="tone-${escapeHtml(resolvedTone)}">${escapeHtml(displayDetailValue(value))}</dd></div>`
}
function populationFrequencySummary(overall,groupMaximum,overallTooltip,groupMaximumTooltip){
  const tooltip=[`Overall AF: ${overallTooltip}`,`Group-max AF: ${groupMaximumTooltip}`].join(' · ');
  return`<div class="population-af-summary" title="${escapeHtml(tooltip)}"><dt>Population AF</dt><dd><span class="population-af-value tone-${variantFactTone('Overall AF',overall)}">${escapeHtml(displayDetailValue(overall))}</span><small>Group max: <span class="population-af-group-value tone-${variantFactTone('Group-max AF',groupMaximum)}">${escapeHtml(displayDetailValue(groupMaximum))}</span></small></dd></div>`
}
function variantSummaryRow(cells){return`<div class="detail-summary-row" style="--detail-summary-columns:${cells.length}">${cells.join('')}</div>`}

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
  $('#variant-detail-body').innerHTML=`<section class="detail-overview"><div class="detail-links">${links.map(([label,url,title])=>`<a href="${escapeHtml(url)}" target="_blank" rel="noopener noreferrer" title="${escapeHtml(title)}">${escapeHtml(label)} ↗</a>`).join('')}</div><dl class="detail-summary">${summaryRows}</dl></section>${groupedEvidenceSection('Clinical & population evidence',[['Clinical',clinicalItems],['Population',populationItems,'None reported',{kind:'population',preferredSourceId:frequency?.sourceId}]],{className:'clinical-population-section',sectionKey:'clinical-population'})}<section class="detail-section transcript-context"><details class="transcript-details collapsible-detail" data-variant-detail-section="transcript-molecular" ${variantDetailOpenSections.has('transcript-molecular')?'open':''}><summary><strong>${escapeHtml(sectionHeading)}</strong>${detailToggleControl()}</summary><div class="transcript-context-body">${unique.length?`<select id="detail-consequence-select" aria-label="Consequence context" title="The selected feature controls the effect and identifier fields below.">${options}</select><div class="selected-transcript-card">${isTranscript?transcriptDetail(selected,metadata):nonTranscriptDetail(selected)}</div>`:'<p class="detail-empty">No molecular consequence was recorded.</p>'}${detail.consequencesTruncated?'<p class="detail-warning">Only the first 1,000 consequences are available.</p>':''}</div></details></section>${groupedEvidenceSection('Predictions & conservation',[['Prediction scores',predictionItems],['Splicing',splicingItems],['Conservation',conservationItems]],{className:'prediction-evidence-section',sectionKey:'predictions-conservation'})}${groupedEvidenceSection('Technical details & provenance',technicalGroups,{className:'technical-provenance-section',extra:provenance,sectionKey:'technical-provenance'})}`;
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
  $('#rows').querySelectorAll('tr[data-allele-id]').forEach(element=>{const row=variants.find(item=>item.alleleId===element.dataset.alleleId);if(!row)return;shown.forEach(([key],index)=>{const cell=element.children[index+2],value=resultColumnRawValue(row,key);if(key.startsWith('evidence:')){const evidenceIndex=Number(key.slice(9)),presentation=tableValuePresentation(key,value,row.evidenceResolution?.[evidenceIndex]);cell.innerHTML=`<span class="table-value tone-${presentation.tone}" title="${escapeHtml(presentation.description)}">${escapeHtml(presentation.display)}</span>`}else if(key==='consequence'||key==='biotype')cell.textContent=readableTerm(value)})});
}

function renderTable(event){renderTableBase(event);enhanceResultGrid()}
