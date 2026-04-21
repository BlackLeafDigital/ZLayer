# StepResult

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Output** | Pointer to **NullableString** | Optional output or error message. | [optional] 
**Status** | **string** | Step outcome: &#x60;\&quot;ok\&quot;&#x60;, &#x60;\&quot;failed\&quot;&#x60;, or &#x60;\&quot;skipped\&quot;&#x60;. | 
**StepName** | **string** | The step name. | 

## Methods

### NewStepResult

`func NewStepResult(status string, stepName string, ) *StepResult`

NewStepResult instantiates a new StepResult object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewStepResultWithDefaults

`func NewStepResultWithDefaults() *StepResult`

NewStepResultWithDefaults instantiates a new StepResult object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetOutput

`func (o *StepResult) GetOutput() string`

GetOutput returns the Output field if non-nil, zero value otherwise.

### GetOutputOk

`func (o *StepResult) GetOutputOk() (*string, bool)`

GetOutputOk returns a tuple with the Output field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOutput

`func (o *StepResult) SetOutput(v string)`

SetOutput sets Output field to given value.

### HasOutput

`func (o *StepResult) HasOutput() bool`

HasOutput returns a boolean if a field has been set.

### SetOutputNil

`func (o *StepResult) SetOutputNil(b bool)`

 SetOutputNil sets the value for Output to be an explicit nil

### UnsetOutput
`func (o *StepResult) UnsetOutput()`

UnsetOutput ensures that no value is present for Output, not even an explicit nil
### GetStatus

`func (o *StepResult) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *StepResult) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *StepResult) SetStatus(v string)`

SetStatus sets Status field to given value.


### GetStepName

`func (o *StepResult) GetStepName() string`

GetStepName returns the StepName field if non-nil, zero value otherwise.

### GetStepNameOk

`func (o *StepResult) GetStepNameOk() (*string, bool)`

GetStepNameOk returns a tuple with the StepName field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStepName

`func (o *StepResult) SetStepName(v string)`

SetStepName sets StepName field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


