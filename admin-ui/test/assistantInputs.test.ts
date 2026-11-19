import assert from 'node:assert/strict';
import test from 'node:test';
import { assistantArguments, walkReports } from '../src/device/assistantInputs.ts';

test('the descriptor editor sends typed nullable objects and arrays and generates a correlation', () => {
  const schema = { required: ['definition','correlation_id'], properties: {
    definition: { anyOf: [{type:'object'},{type:'null'}] }, correlation_id:{type:'string'},
    target: {type:['string','null']}, offsets: {type:'array'}, count:{type:'integer'}, choice:{enum:[0,1]}
  } };
  const parse = (values: Record<string,string>) => assistantArguments(schema,values,()=> 'review-1');
  assert.deepEqual(parse({definition:'{"enabled":true}',target:'null',offsets:'[0,5]',count:'0',choice:'0'}),
    {definition:{enabled:true},correlation_id:'review-1',target:null,offsets:[0,5],count:0,choice:0});
  assert.equal(parse({definition:'null'}).definition,null);
  assert.throws(()=>parse({definition:'false'}),/valid object or null/);
  assert.throws(()=>parse({definition:'null',count:'1.5'}),/valid integer/);
});
test('walk readback retains all explicit positive and negative resident observations until a new walk', () => {
  const on = walkReports({}, {device_id:'hall',power_on:true});
  const off = walkReports(on.reports,{device_id:'landing',power_on:false});
  assert.deepEqual(walkReports(off.reports).observations,[{device_id:'hall',power_on:true},{device_id:'landing',power_on:false}]);
  assert.deepEqual(walkReports({}).observations,[]);
});
