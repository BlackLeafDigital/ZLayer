# SyncResourceResult

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Action** | **string** | Action taken: &#x60;\&quot;create\&quot;&#x60;, &#x60;\&quot;update\&quot;&#x60;, &#x60;\&quot;delete\&quot;&#x60;, or &#x60;\&quot;skip\&quot;&#x60;. | 
**Error** | Pointer to **NullableString** | Optional error message (&#x60;status &#x3D;&#x3D; \&quot;error\&quot;&#x60;) or skip reason (&#x60;action &#x3D;&#x3D; \&quot;skip\&quot;&#x60;). Omitted on successful &#x60;\&quot;ok\&quot;&#x60; results. | [optional] 
**Kind** | **string** | Resource kind: &#x60;\&quot;deployment\&quot;&#x60;, &#x60;\&quot;job\&quot;&#x60;, &#x60;\&quot;cron\&quot;&#x60;, or other. | 
**Resource** | **string** | The source manifest path (or remote resource name for deletions). | 
**Status** | **string** | Outcome status: &#x60;\&quot;ok\&quot;&#x60; or &#x60;\&quot;error\&quot;&#x60;. | 

## Methods

### NewSyncResourceResult

`func NewSyncResourceResult(action string, kind string, resource string, status string, ) *SyncResourceResult`

NewSyncResourceResult instantiates a new SyncResourceResult object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewSyncResourceResultWithDefaults

`func NewSyncResourceResultWithDefaults() *SyncResourceResult`

NewSyncResourceResultWithDefaults instantiates a new SyncResourceResult object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAction

`func (o *SyncResourceResult) GetAction() string`

GetAction returns the Action field if non-nil, zero value otherwise.

### GetActionOk

`func (o *SyncResourceResult) GetActionOk() (*string, bool)`

GetActionOk returns a tuple with the Action field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAction

`func (o *SyncResourceResult) SetAction(v string)`

SetAction sets Action field to given value.


### GetError

`func (o *SyncResourceResult) GetError() string`

GetError returns the Error field if non-nil, zero value otherwise.

### GetErrorOk

`func (o *SyncResourceResult) GetErrorOk() (*string, bool)`

GetErrorOk returns a tuple with the Error field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetError

`func (o *SyncResourceResult) SetError(v string)`

SetError sets Error field to given value.

### HasError

`func (o *SyncResourceResult) HasError() bool`

HasError returns a boolean if a field has been set.

### SetErrorNil

`func (o *SyncResourceResult) SetErrorNil(b bool)`

 SetErrorNil sets the value for Error to be an explicit nil

### UnsetError
`func (o *SyncResourceResult) UnsetError()`

UnsetError ensures that no value is present for Error, not even an explicit nil
### GetKind

`func (o *SyncResourceResult) GetKind() string`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *SyncResourceResult) GetKindOk() (*string, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *SyncResourceResult) SetKind(v string)`

SetKind sets Kind field to given value.


### GetResource

`func (o *SyncResourceResult) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *SyncResourceResult) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *SyncResourceResult) SetResource(v string)`

SetResource sets Resource field to given value.


### GetStatus

`func (o *SyncResourceResult) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *SyncResourceResult) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *SyncResourceResult) SetStatus(v string)`

SetStatus sets Status field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


