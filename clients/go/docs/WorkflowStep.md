# WorkflowStep

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Action** | [**WorkflowAction**](WorkflowAction.md) | The action to perform. | 
**Name** | **string** | Step name (display label). | 
**OnFailure** | Pointer to **NullableString** | Name of another step (or task id) to run on failure. | [optional] 

## Methods

### NewWorkflowStep

`func NewWorkflowStep(action WorkflowAction, name string, ) *WorkflowStep`

NewWorkflowStep instantiates a new WorkflowStep object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewWorkflowStepWithDefaults

`func NewWorkflowStepWithDefaults() *WorkflowStep`

NewWorkflowStepWithDefaults instantiates a new WorkflowStep object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAction

`func (o *WorkflowStep) GetAction() WorkflowAction`

GetAction returns the Action field if non-nil, zero value otherwise.

### GetActionOk

`func (o *WorkflowStep) GetActionOk() (*WorkflowAction, bool)`

GetActionOk returns a tuple with the Action field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAction

`func (o *WorkflowStep) SetAction(v WorkflowAction)`

SetAction sets Action field to given value.


### GetName

`func (o *WorkflowStep) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *WorkflowStep) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *WorkflowStep) SetName(v string)`

SetName sets Name field to given value.


### GetOnFailure

`func (o *WorkflowStep) GetOnFailure() string`

GetOnFailure returns the OnFailure field if non-nil, zero value otherwise.

### GetOnFailureOk

`func (o *WorkflowStep) GetOnFailureOk() (*string, bool)`

GetOnFailureOk returns a tuple with the OnFailure field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOnFailure

`func (o *WorkflowStep) SetOnFailure(v string)`

SetOnFailure sets OnFailure field to given value.

### HasOnFailure

`func (o *WorkflowStep) HasOnFailure() bool`

HasOnFailure returns a boolean if a field has been set.

### SetOnFailureNil

`func (o *WorkflowStep) SetOnFailureNil(b bool)`

 SetOnFailureNil sets the value for OnFailure to be an explicit nil

### UnsetOnFailure
`func (o *WorkflowStep) UnsetOnFailure()`

UnsetOnFailure ensures that no value is present for OnFailure, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


