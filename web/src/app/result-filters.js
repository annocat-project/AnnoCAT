export function remapEvidenceFilterRules(rules,previousCatalog,nextCatalog){
  return rules.map(rule=>{
    if(!rule.column?.startsWith('evidence:'))return rule;
    const field=previousCatalog[Number(rule.column.slice(9))];
    if(!field)return null;
    const index=nextCatalog.findIndex(candidate=>candidate.scope===field.scope&&candidate.sourceId===field.sourceId&&candidate.fieldPath===field.fieldPath);
    return index<0?null:{...rule,column:`evidence:${index}`};
  }).filter(Boolean);
}

export function createResultFilters({
  $,escapeHtml,coreFilterColumns,filterOperators,numericFilterOperators,FILTER_PRESET_STORAGE_KEY,
  selectableEvidenceEntries,coreColumnPresentation,evidenceFieldPresentation,resourceTitle,
  resetResultPages,clearVariantSelection,openCompletedRun,getState
}){
  let humanReadableColumnNames=true,resultFieldCatalog=[],selectionMode='explicit',currentResultRun=null,profileLinkedField=null;
  $('#result-filters')?.classList.add('fui-popover--nested-content');
  function syncState(){({humanReadableColumnNames,resultFieldCatalog,selectionMode,currentResultRun}=getState())}

  function likelyNumericEvidenceField(field){
    const name=String(field?.fieldPath||'').toLowerCase();
    return['integer','number'].includes(field?.valueType)||/(^|_)(score|rankscore|phred|raw|af|faf|ac|an|nhomalt|count|frequency|percentile|distance|depth|dp|gq|mq|fs|sor|qd)(_|$)/.test(name)||/(phylop|gerp|spliceai|cadd|revel|primateai|alphamissense)/.test(name)&&!/(pred|prediction|class|label|id)$/.test(name);
  }
  function filterColumnDefinition(value){
    syncState();
    if(value?.startsWith('evidence:')){
      const index=Number(value.slice(9)),field=resultFieldCatalog[index],presentation=field?evidenceFieldPresentation(field):null;
      return field?{key:value,label:`${resourceTitle(field.sourceId)} · ${presentation.label}`,type:field.categorical?'text':likelyNumericEvidenceField(field)?'number':field.valueType==='boolean'?'boolean':'text',categorical:field.categorical||null,field,index}:null;
    }
    return coreFilterColumns.find(column=>column.key===value)||null;
  }
  function categoryKey(value){return String(value??'').trim().toLowerCase().replace(/[_-]/g,' ').replace(/\s+/g,' ')}
  function categoryChoices(definition,selected=[]){
    const choices=new Map();
    const add=item=>{
      const value=typeof item==='string'?item:item?.value,label=typeof item==='string'?item.replaceAll('_',' '):item?.label||item?.value;
      if(value==null||String(value).trim()==='')return;
      const key=categoryKey(value),candidate={value:String(value),label:String(label)};
      if(!choices.has(key))choices.set(key,candidate);
    };
    (definition?.categorical?.values||[]).forEach(add);
    (definition?.categorical?.observedValues||[]).forEach(add);
    selected.forEach(add);
    return[...choices.values()].sort((a,b)=>a.label.localeCompare(b.label,undefined,{sensitivity:'base'}));
  }
  function selectedCategoricalValues(rule={}){
    if(Array.isArray(rule.values))return rule.values.map(String);
    return String(rule.value||'').split(',').map(value=>value.trim()).filter(Boolean);
  }
  function rowFilterValue(row){
    const control=row.querySelector('[data-filter-value]');
    if(!row.querySelector('[data-filter-categorical]'))return{value:control?.value.trim()||''};
    let values=[];
    try{values=JSON.parse(control?.value||'[]')}catch{}
    return{value:'',values:Array.isArray(values)?values:[],includeMissing:Boolean(row.querySelector('[data-filter-include-missing]')?.checked)};
  }
  function resultFilterRules(){
    return[...$('#filter-rules').querySelectorAll('.filter-rule:not(.filter-rule--profile)')].map(row=>{
      const value=rowFilterValue(row);
      return{column:row.querySelector('[data-filter-column]').value,operator:row.querySelector('[data-filter-operator]').value,...value,origin:row.dataset.filterOrigin||''};
    }).filter(rule=>rule.column&&rule.operator&&(rule.value!==''||rule.values?.length||rule.includeMissing));
  }
  function resultFilterParameters(){
    const filterRules=[],evidenceFilters=[];
    resultFilterRules().forEach(rule=>{
      const definition=filterColumnDefinition(rule.column);
      if(!definition)return;
      const queryRule={operator:rule.operator,value:rule.value||''};
      if(rule.values)queryRule.values=rule.values;
      if(rule.includeMissing!==undefined)queryRule.includeMissing=rule.includeMissing;
      if(definition.field)evidenceFilters.push({index:definition.index,...queryRule,value2:''});
      else filterRules.push({column:rule.column,...queryRule});
    });
    if(profileLinkedField){
      const index=resultFieldCatalog.findIndex(field=>field.scope===profileLinkedField.scope&&field.sourceId===profileLinkedField.sourceId&&field.fieldPath===profileLinkedField.fieldPath);
      if(index>=0)evidenceFilters.push({index,operator:'equals',value:'true',value2:''});
    }
    return{filterRules,evidenceFilters};
  }
  function filterColumnChoices(){
    syncState();
    const choices=coreFilterColumns.map(column=>{const presentation=coreColumnPresentation(column.key,column.label);return{key:column.key,label:humanReadableColumnNames?presentation.readableLabel:column.key,raw:column.key,source:'Core annotation',description:presentation.description}});
    selectableEvidenceEntries().forEach(({field,index})=>{const presentation=evidenceFieldPresentation(field);choices.push({key:`evidence:${index}`,label:humanReadableColumnNames?presentation.label:field.fieldPath,raw:field.fieldPath,source:resourceTitle(field.sourceId||'Other evidence'),description:presentation.description})});
    return choices;
  }
  function filterColumnPicker(selected){
    const choices=filterColumnChoices(),current=choices.find(choice=>choice.key===selected)||choices[0],groups=new Map();
    choices.forEach(choice=>{if(!groups.has(choice.source))groups.set(choice.source,[]);groups.get(choice.source).push(choice)});
    const options=[...groups.entries()].map(([source,items])=>`<section data-filter-column-option-group><strong class="fui-menu-group__label">${escapeHtml(source)}</strong>${items.map(choice=>`<button type="button" class="fui-menu-item fui-menu-item--described" role="option" data-filter-column-option="${escapeHtml(choice.key)}" data-filter-column-search-text="${escapeHtml(`${source} ${choice.label} ${choice.raw} ${choice.description}`.toLowerCase())}" aria-selected="${choice.key===current.key}"><span class="fui-menu-item__content"><strong class="fui-menu-item__title" data-filter-column-option-label>${escapeHtml(choice.label)}</strong><small class="fui-menu-item__description">${escapeHtml(choice.description)}</small></span><code class="fui-menu-item__metadata">${escapeHtml(choice.raw)}</code></button>`).join('')}</section>`).join('');
    return`<div class="filter-column-picker"><input type="hidden" data-filter-column value="${escapeHtml(current.key)}"><button type="button" class="fui-button fui-select-trigger filter-column-trigger" data-filter-column-toggle aria-haspopup="listbox" aria-expanded="false" title="${escapeHtml(current.description)}"><span data-filter-column-label>${escapeHtml(current.label)}</span><svg class="ui-icon fui-select-trigger__icon" aria-hidden="true"><use href="#icon-chevron-down"></use></svg></button><div class="filter-column-options fui-popover fui-popover--listbox hidden" role="listbox"><input type="search" class="fui-input filter-picker-search" data-filter-column-search aria-label="Search filter columns" placeholder="Search columns, sources, descriptions, or raw keys"><div class="filter-column-option-list">${options}</div></div></div>`;
  }
  function categoricalValueControl(definition,rule={}){
    const selected=selectedCategoricalValues(rule),selectedKeys=new Set(selected.map(categoryKey)),choices=categoryChoices(definition,selected),summary=selected.length?`${selected.length} selected`:rule.includeMissing?'Missing values':'Choose values';
    const options=choices.length?choices.map(choice=>`<label class="fui-menu-item categorical-filter-option" data-category-search-text="${escapeHtml(`${choice.label} ${choice.value}`.toLowerCase())}"><input class="fui-checkbox" type="checkbox" value="${escapeHtml(choice.value)}" ${selectedKeys.has(categoryKey(choice.value))?'checked':''}><div>${escapeHtml(choice.label)}</div></label>`).join(''):'<p class="categorical-filter-empty fui-caption">No values loaded.</p>',observed=Boolean(definition.categorical?.observedValues?.length),heading=observed?'Available in this result':'Supported values';
    const exactOption=definition.categorical?.canDiscover===false?'':'<button type="button" class="fui-menu-item categorical-filter-exact hidden" data-enter-category></button>';
    return`<div class="categorical-filter" data-filter-categorical><input type="hidden" data-filter-value value="${escapeHtml(JSON.stringify(selected))}"><button type="button" class="fui-button fui-select-trigger categorical-filter-trigger" data-category-toggle aria-haspopup="listbox" aria-expanded="false"><span data-category-summary>${escapeHtml(summary)}</span><svg class="ui-icon fui-select-trigger__icon" aria-hidden="true"><use href="#icon-chevron-down"></use></svg></button><div class="categorical-filter-options fui-popover fui-popover--listbox hidden"><input type="search" class="fui-input filter-picker-search" data-category-search aria-label="Search or enter an exact value" placeholder="Search or enter an exact value"><div class="categorical-filter-heading"><strong class="fui-menu-group__label">${heading}</strong><span class="fui-caption">${choices.length} ${choices.length===1?'value':'values'}</span></div><div class="categorical-filter-list" role="listbox" aria-multiselectable="true">${options}</div>${exactOption}<div class="categorical-filter-actions">${definition.categorical?.canDiscover?'<button type="button" class="fui-menu-item" data-discover-categories>Find other values in this result</button>':''}<label class="fui-menu-item categorical-filter-missing"><input class="fui-checkbox" type="checkbox" data-filter-include-missing ${rule.includeMissing?'checked':''}><div>Include not reported</div></label><small data-category-status></small></div></div></div>`;
  }
  function filterValueControl(definition,rule={},operator=''){
    if(definition?.categorical)return categoricalValueControl(definition,rule);
    const value=rule.value||'';
    if(definition?.type==='boolean')return`<select class="fui-select" data-filter-value aria-label="Filter value"><option value="true" ${value==='true'?'selected':''}>Yes</option><option value="false" ${value==='false'?'selected':''}>No</option></select>`;
    const list=['in','not_in'].includes(operator),placeholder=list?(definition?.type==='number'?'10, 20, 30':'Enter comma-separated values'):definition?.key==='gene'?'BRCA1':definition?.type==='number'?'Enter a number':'Enter a value';
    return`<input class="fui-input" data-filter-value value="${escapeHtml(value)}" placeholder="${escapeHtml(placeholder)}" ${definition?.type==='number'?'inputmode="decimal"':''}>`;
  }
  function defaultFilterOperator(definition){
    if(definition?.categorical)return'in';
    const name=`${definition?.key||''} ${definition?.field?.sourceId||''} ${definition?.field?.fieldPath||''}`.toLowerCase();
    if(definition?.type==='boolean')return'equals';
    if(definition?.type==='number'){if(/(^|[^a-z])(af|faf)([^a-z]|$)|frequency|sift/.test(name))return'lte';if(/quality|score|phred|phylop|gerp|revel|cadd|spliceai|primateai|alphamissense/.test(name))return'gte';return'equals'}
    if(definition?.key==='gene')return'in';
    if(/consequence|phenotype|condition|disease|significance/.test(name))return'contains';
    return'equals';
  }
  function filterOperatorOptions(definition,selected){
    const allowed=new Set(definition?.categorical?['in','not_in']:definition?.type==='number'?['equals','not_equals','gt','gte','lt','lte','in','not_in']:definition?.type==='boolean'?['equals','not_equals']:['equals','not_equals','contains','not_contains','in','not_in']),choice=allowed.has(selected)?selected:defaultFilterOperator(definition);
    return filterOperators.filter(([value])=>allowed.has(value)).map(([value,label])=>`<option value="${value}" ${choice===value?'selected':''}>${escapeHtml(definition?.key==='consequence'&&value==='in'?'is any of':definition?.key==='consequence'&&value==='not_in'?'is none of':label)}</option>`).join('');
  }
  function addFilterRule(rule={column:'gene',operator:'in',value:''},render=true){
    const definition=filterColumnDefinition(rule.column)||coreFilterColumns.find(column=>column.key==='gene');
    $('#filter-rules').insertAdjacentHTML('beforeend',`<div class="filter-rule" ${rule.origin?`data-filter-origin="${escapeHtml(rule.origin)}"`:''}>${filterColumnPicker(definition.key)}<select class="fui-select" data-filter-operator aria-label="Filter comparison">${filterOperatorOptions(definition,rule.operator)}</select><span class="filter-rule-value">${filterValueControl(definition,rule,rule.operator)}</span><button type="button" class="fui-button fui-button--icon fui-button--subtle" data-remove-filter aria-label="Remove filter"><svg class="ui-icon" aria-hidden="true"><use href="#icon-close"></use></svg></button></div>`);
    if(render)bindFilterRule($('#filter-rules').lastElementChild);
  }
  function profileLinkedFilterMarkup(){return profileLinkedField?'<div class="filter-rule filter-rule--profile" aria-label="Applied gene-list filter"><input class="fui-input" value="Gene list" readonly tabindex="-1" aria-label="Filter field"><input class="fui-input" value="is applied" readonly tabindex="-1" aria-label="Filter comparison"><span class="filter-rule-value"><input class="fui-input" value="Show matching genes only" readonly tabindex="-1" aria-label="Filter value"></span></div>':''}
  function filterFilterColumnOptions(picker,query){const normalized=query.trim().toLowerCase();picker.querySelectorAll('[data-filter-column-option-group]').forEach(group=>{const options=[...group.querySelectorAll('[data-filter-column-option]')];options.forEach(option=>option.classList.toggle('hidden',Boolean(normalized)&&!option.dataset.filterColumnSearchText.includes(normalized)));group.classList.toggle('hidden',options.every(option=>option.classList.contains('hidden')))})}
  function closeFilterColumnPicker(picker){picker.querySelector('.filter-column-options').classList.add('hidden');picker.querySelector('[data-filter-column-toggle]').setAttribute('aria-expanded','false')}
  function refreshCategorySummary(row){
    const control=row.querySelector('[data-filter-value]'),values=[...row.querySelectorAll('.categorical-filter-list input:checked')].map(input=>input.value),includeMissing=row.querySelector('[data-filter-include-missing]').checked;
    control.value=JSON.stringify(values);
    row.querySelector('[data-category-summary]').textContent=values.length?`${values.length} selected`:includeMissing?'Missing values':'Choose values';
  }
  function bindCategoricalControl(row){
    const control=row.querySelector('[data-filter-categorical]');
    if(!control)return;
    const trigger=control.querySelector('[data-category-toggle]'),menu=control.querySelector('.categorical-filter-options'),search=control.querySelector('[data-category-search]'),exactOption=control.querySelector('[data-enter-category]');
    const filterOptions=()=>{
      const exact=search.value.trim(),query=exact.toLowerCase(),exactKey=categoryKey(exact);
      let hasExactMatch=false,visibleCount=0;
      control.querySelectorAll('[data-category-search-text]').forEach(option=>{
        const input=option.querySelector('input');
        const visible=!query||option.dataset.categorySearchText.includes(query);
        option.classList.toggle('hidden',!visible);
        if(visible)visibleCount++;
        if(exactKey&&categoryKey(input?.value)===exactKey)hasExactMatch=true;
      });
      exactOption?.classList.toggle('hidden',!exact||hasExactMatch||visibleCount>0);
      if(exactOption)exactOption.textContent=exact&&!hasExactMatch&&visibleCount===0?`Use exact value “${exact}”`:'';
    };
    const addExactValue=()=>{
      const exact=search.value.trim();
      if(!exact)return;
      const rule=rowFilterValue(row),values=rule.values||[],status=control.querySelector('[data-category-status]');
      if(new TextEncoder().encode(exact).length>1024){status.textContent='The value is too long';return}
      if(!values.some(value=>categoryKey(value)===categoryKey(exact))){if(values.length>=100){status.textContent='Select at most 100 values';return}values.push(exact)}
      const definition=filterColumnDefinition(row.querySelector('[data-filter-column]').value);
      replaceValueControl(row,definition,{...rule,values},row.querySelector('[data-filter-operator]').value);filterRulesChanged();
    };
    trigger.addEventListener('click',()=>{const open=menu.classList.contains('hidden');document.querySelectorAll('.categorical-filter-options').forEach(other=>{if(other!==menu)other.classList.add('hidden')});menu.classList.toggle('hidden',!open);trigger.setAttribute('aria-expanded',String(open));if(open){search.value='';filterOptions();requestAnimationFrame(()=>search.focus())}});
    search.addEventListener('input',filterOptions);
    search.addEventListener('keydown',event=>{
      if(event.key!=='Enter')return;
      const exact=search.value.trim();
      if(!exact)return;
      event.preventDefault();
      const inputs=[...control.querySelectorAll('.categorical-filter-option input')],exactMatch=inputs.find(input=>categoryKey(input.value)===categoryKey(exact)),visible=inputs.filter(input=>!input.closest('.categorical-filter-option').classList.contains('hidden')),match=exactMatch||(visible.length===1?visible[0]:null);
      if(match){match.checked=true;refreshCategorySummary(row);filterRulesChanged();search.value='';filterOptions();return}
      if(exactOption&&visible.length===0)addExactValue();
    });
    control.querySelector('.categorical-filter-list').addEventListener('change',()=>{refreshCategorySummary(row);filterRulesChanged()});
    control.querySelector('[data-filter-include-missing]').addEventListener('change',()=>{refreshCategorySummary(row);filterRulesChanged()});
    exactOption?.addEventListener('click',addExactValue);
    control.querySelector('[data-discover-categories]')?.addEventListener('click',async()=>{
      syncState();
      const status=control.querySelector('[data-category-status]'),definition=filterColumnDefinition(row.querySelector('[data-filter-column]').value),runId=currentResultRun?.id||currentResultRun;
      if(!runId||!definition)return;
      status.textContent='Finding values…';
      try{
        const query=definition.field?`evidenceIndex=${definition.index}`:`coreColumn=${encodeURIComponent(definition.key)}`,response=await fetch(`/api/runs/${encodeURIComponent(runId)}/filter-values?${query}`),body=await response.json();
        if(!response.ok)throw new Error(body.error||'Values could not be loaded');
        definition.categorical.observedValues=body.values||[];
        const rule={...rowFilterValue(row),operator:row.querySelector('[data-filter-operator]').value};
        row.querySelector('.filter-rule-value').innerHTML=filterValueControl(definition,rule,rule.operator);
        bindCategoricalControl(row);
      }catch(error){status.textContent=error.message}
    });
  }
  function replaceValueControl(row,definition,rule,operator){row.querySelector('.filter-rule-value').innerHTML=filterValueControl(definition,rule,operator);bindCategoricalControl(row)}
  function bindFilterRule(row){
    const picker=row.querySelector('.filter-column-picker'),value=picker.querySelector('[data-filter-column]'),toggle=picker.querySelector('[data-filter-column-toggle]'),menu=picker.querySelector('.filter-column-options'),search=picker.querySelector('[data-filter-column-search]'),operator=row.querySelector('[data-filter-operator]');
    bindCategoricalControl(row);
    toggle.addEventListener('click',()=>{const open=menu.classList.contains('hidden');document.querySelectorAll('.filter-column-picker').forEach(other=>{if(other!==picker)closeFilterColumnPicker(other)});menu.classList.toggle('hidden',!open);toggle.setAttribute('aria-expanded',String(open));if(open){search.value='';filterFilterColumnOptions(picker,'');requestAnimationFrame(()=>search.focus())}});
    search.addEventListener('input',()=>filterFilterColumnOptions(picker,search.value));
    search.addEventListener('keydown',event=>{event.stopPropagation();if(event.key==='Escape'){event.preventDefault();closeFilterColumnPicker(picker);toggle.focus()}else if(event.key==='Enter'){const option=picker.querySelector('[data-filter-column-option]:not(.hidden)');if(option){event.preventDefault();option.click()}}});
    menu.addEventListener('click',event=>{
      const option=event.target.closest('[data-filter-column-option]');if(!option)return;
      const previousDefinition=filterColumnDefinition(value.value),previous=rowFilterValue(row);
      value.value=option.dataset.filterColumnOption;
      const choice=filterColumnChoices().find(item=>item.key===value.value),definition=filterColumnDefinition(value.value);
      toggle.querySelector('[data-filter-column-label]').textContent=option.querySelector('[data-filter-column-option-label]').textContent;toggle.title=choice?.description||'';
      picker.querySelectorAll('[data-filter-column-option]').forEach(item=>item.setAttribute('aria-selected',String(item===option)));closeFilterColumnPicker(picker);
      operator.innerHTML=filterOperatorOptions(definition,defaultFilterOperator(definition));
      replaceValueControl(row,definition,previousDefinition?.categorical===definition?.categorical?previous:{value:''},operator.value);filterRulesChanged();
    });
    operator.addEventListener('change',()=>{const definition=filterColumnDefinition(value.value),previous=rowFilterValue(row);replaceValueControl(row,definition,previous,operator.value)});
    row.querySelector('[data-remove-filter]').addEventListener('click',event=>{event.stopPropagation();row.remove();if(!$('#filter-rules').querySelector('.filter-rule:not(.filter-rule--profile)'))addFilterRule();filterRulesChanged()});
  }
  function validateResultFilters(){
    for(const rule of resultFilterRules()){
      const definition=filterColumnDefinition(rule.column);
      if(rule.values){if(rule.values.length>100)return`“${definition?.label||rule.column}” supports at most 100 values`;continue}
      if(numericFilterOperators.has(rule.operator)&&(!Number.isFinite(Number(rule.value))||rule.value===''))return`“${definition?.label||rule.column}” needs a valid number for ${rule.operator==='gte'?'≥':rule.operator==='lte'?'≤':rule.operator==='gt'?'>':'<'}`;
      if(definition?.type==='number'&&['in','not_in'].includes(rule.operator)&&rule.value.split(',').some(value=>!value.trim()||!Number.isFinite(Number(value))))return`“${definition.label}” needs a comma-separated list of numbers`;
    }
    return'';
  }
  function renderFilterRules(rules=resultFilterRules()){const host=$('#filter-rules');host.innerHTML=profileLinkedFilterMarkup();(rules.length?rules:[{column:'gene',operator:'in',value:''}]).forEach(rule=>addFilterRule(rule,false));host.querySelectorAll('.filter-rule:not(.filter-rule--profile)').forEach(bindFilterRule)}
  function captureFilterRules(){return{rules:resultFilterRules(),catalog:resultFieldCatalog}}
  function restoreFilterRules(snapshot){renderFilterRules(remapEvidenceFilterRules(snapshot?.rules||[],snapshot?.catalog||[],resultFieldCatalog))}
  function filterRulesChanged(){syncState();resetResultPages();if(selectionMode==='filtered')clearVariantSelection(true);$('#filter-message').textContent='Filters changed — apply to update results'}
  function clearResultFilters(refresh=true){syncState();profileLinkedField=null;renderFilterRules([{column:'gene',operator:'in',value:''}]);$('#filter-message').textContent='';if(refresh&&currentResultRun)openCompletedRun(currentResultRun,0)}
  function savedFilterPresets(){try{const value=JSON.parse(localStorage.getItem(FILTER_PRESET_STORAGE_KEY)||'[]');return Array.isArray(value)?value.slice(0,50):[]}catch{return[]}}
  function refreshFilterPresetSelector(selected=''){const presets=savedFilterPresets(),selector=$('#saved-filter-presets');selector.innerHTML='<option value="">Choose a saved filter…</option>'+presets.map((preset,index)=>`<option value="${index}" ${String(index)===String(selected)?'selected':''}>${escapeHtml(preset.name)}</option>`).join('')}
  function presetRules(){return resultFilterRules().filter(rule=>rule.origin!=='phenotype-profile').map(rule=>{const definition=filterColumnDefinition(rule.column),stored={operator:rule.operator,value:rule.value||''};if(rule.values)stored.values=rule.values;if(rule.includeMissing!==undefined)stored.includeMissing=rule.includeMissing;return definition?.field?{column:'evidence',...stored,field:{scope:definition.field.scope,sourceId:definition.field.sourceId,fieldPath:definition.field.fieldPath}}:{column:rule.column,...stored}})}
  function setProfileLinkedFilter(field,enabled){profileLinkedField=enabled&&field?{scope:field.scope,sourceId:field.sourceId,fieldPath:field.fieldPath}:null;renderFilterRules(resultFilterRules().filter(rule=>rule.origin!=='phenotype-profile'));resetResultPages();if(selectionMode==='filtered')clearVariantSelection(true)}
  function saveFilterPreset(){const rules=presetRules();if(!rules.length){$('#filter-message').textContent='Add at least one complete filter before you save it';return}const name=prompt('Saved filter name');if(!name?.trim())return;const presets=savedFilterPresets(),clean=name.trim().slice(0,80),existing=presets.findIndex(preset=>preset.name.toLowerCase()===clean.toLowerCase()),preset={name:clean,rules};if(existing>=0)presets[existing]=preset;else presets.push(preset);localStorage.setItem(FILTER_PRESET_STORAGE_KEY,JSON.stringify(presets.slice(0,50)));refreshFilterPresetSelector(existing>=0?existing:presets.length-1);$('#filter-message').textContent=`Saved “${clean}” for all results`}
  function loadFilterPreset(){syncState();const selected=$('#saved-filter-presets').value;if(selected==='')return;const index=Number(selected),preset=savedFilterPresets()[index];if(!preset)return;let unavailable=0;const rules=preset.rules.map(rule=>{if(rule.column!=='evidence')return rule;const fieldIndex=resultFieldCatalog.findIndex(field=>field.scope===rule.field?.scope&&field.sourceId===rule.field?.sourceId&&field.fieldPath===rule.field?.fieldPath);if(fieldIndex<0){unavailable++;return null}return{...rule,column:`evidence:${fieldIndex}`}}).filter(Boolean);renderFilterRules(rules);if(selectionMode==='filtered')clearVariantSelection(true);$('#filter-message').textContent=unavailable?`${unavailable} saved annotation field${unavailable===1?' is':'s are'} not available in this result`:`Loaded “${preset.name}”`}
  function deleteFilterPreset(){const selected=$('#saved-filter-presets').value;if(selected==='')return;const index=Number(selected),presets=savedFilterPresets();if(!presets[index])return;const [removed]=presets.splice(index,1);localStorage.setItem(FILTER_PRESET_STORAGE_KEY,JSON.stringify(presets));refreshFilterPresetSelector();$('#filter-message').textContent=`Deleted “${removed.name}”`}

  return{
    resultFilterParameters:(...args)=>{syncState();return resultFilterParameters(...args)},addFilterRule:(...args)=>{syncState();return addFilterRule(...args)},closeFilterColumnPicker:(...args)=>{syncState();return closeFilterColumnPicker(...args)},validateResultFilters:(...args)=>{syncState();return validateResultFilters(...args)},renderFilterRules:(...args)=>{syncState();return renderFilterRules(...args)},captureFilterRules:(...args)=>{syncState();return captureFilterRules(...args)},restoreFilterRules:(...args)=>{syncState();return restoreFilterRules(...args)},filterRulesChanged:(...args)=>{syncState();return filterRulesChanged(...args)},clearResultFilters:(...args)=>{syncState();return clearResultFilters(...args)},refreshFilterPresetSelector:(...args)=>{syncState();return refreshFilterPresetSelector(...args)},saveFilterPreset:(...args)=>{syncState();return saveFilterPreset(...args)},loadFilterPreset:(...args)=>{syncState();return loadFilterPreset(...args)},deleteFilterPreset:(...args)=>{syncState();return deleteFilterPreset(...args)},setProfileLinkedFilter:(...args)=>{syncState();return setProfileLinkedFilter(...args)}
  };
}
