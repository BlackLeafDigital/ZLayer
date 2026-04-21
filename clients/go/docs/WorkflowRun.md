# WorkflowRun

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**FinishedAt** | Pointer to **NullableString** | When the run finished (&#x60;None&#x60; if still running). | [optional] 
**Id** | **string** | UUID identifier of this run. | 
**StartedAt** | **string** | When the run started. | 
**Status** | [**WorkflowRunStatus**](WorkflowRunStatus.md) | Overall run status. | 
**StepResults** | [**[]StepResult**](StepResult.md) | Per-step results. | 
**WorkflowId** | **string** | The workflow that was executed. | 

## Methods

### NewWorkflowRun

`func NewWorkflowRun(id string, startedAt string, status WorkflowRunStatus, stepResults []StepResult, workflowId string, ) *WorkflowRun`

NewWorkflowRun instantiates a new WorkflowRun object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewWorkflowRunWithDefaults

`func NewWorkflowRunWithDefaults() *WorkflowRun`

NewWorkflowRunWithDefaults instantiates a new WorkflowRun object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetFinishedAt

`func (o *WorkflowRun) GetFinishedAt() string`

GetFinishedAt returns the FinishedAt field if non-nil, zero value otherwise.

### GetFinishedAtOk

`func (o *WorkflowRun) GetFinishedAtOk() (*string, bool)`

GetFinishedAtOk returns a tuple with the FinishedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetFinishedAt

`func (o *WorkflowRun) SetFinishedAt(v string)`

SetFinishedAt sets FinishedAt field to given value.

### HasFinishedAt

`func (o *WorkflowRun) HasFinishedAt() bool`

HasFinishedAt returns a boolean if a field has been set.

### SetFinishedAtNil

`func (o *WorkflowRun) SetFinishedAtNil(b bool)`

 SetFinishedAtNil sets the value for FinishedAt to be an explicit nil

### UnsetFinishedAt
`func (o *WorkflowRun) UnsetFinishedAt()`

UnsetFinishedAt ensures that no value is present for FinishedAt, not even an explicit nil
### GetId

`func (o *WorkflowRun) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *WorkflowRun) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *WorkflowRun) SetId(v string)`

SetId sets Id field to given value.


### GetStartedAt

`func (o *WorkflowRun) GetStartedAt() string`

GetStartedAt returns the StartedAt field if non-nil, zero value otherwise.

### GetStartedAtOk

`func (o *WorkflowRun) GetStartedAtOk() (*string, bool)`

GetStartedAtOk returns a tuple with the StartedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStartedAt

`func (o *WorkflowRun) SetStartedAt(v string)`

SetStartedAt sets StartedAt field to given value.


### GetStatus

`func (o *WorkflowRun) GetStatus() WorkflowRunStatus`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *WorkflowRun) GetStatusOk() (*WorkflowRunStatus, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *WorkflowRun) SetStatus(v WorkflowRunStatus)`

SetStatus sets Status field to given value.


### GetStepResults

`func (o *WorkflowRun) GetStepResults() []StepResult`

GetStepResults returns the StepResults field if non-nil, zero value otherwise.

### GetStepResultsOk

`func (o *WorkflowRun) GetStepResultsOk() (*[]StepResult, bool)`

GetStepResultsOk returns a tuple with the StepResults field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStepResults

`func (o *WorkflowRun) SetStepResults(v []StepResult)`

SetStepResults sets StepResults field to given value.


### GetWorkflowId

`func (o *WorkflowRun) GetWorkflowId() string`

GetWorkflowId returns the WorkflowId field if non-nil, zero value otherwise.

### GetWorkflowIdOk

`func (o *WorkflowRun) GetWorkflowIdOk() (*string, bool)`

GetWorkflowIdOk returns a tuple with the WorkflowId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWorkflowId

`func (o *WorkflowRun) SetWorkflowId(v string)`

SetWorkflowId sets WorkflowId field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


