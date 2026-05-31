# WorkerSummary

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**ApiAddr** | **string** | Worker&#39;s API/health address (host:port). | 
**Id** | **int64** | Worker&#39;s assigned node id. | 
**Labels** | Pointer to **map[string]string** | Labels declared during Register. | [optional] 
**LastSeenUnixSecs** | **int64** | Last time the leader observed the worker. | 
**Os** | **string** | Worker&#39;s reported OS. | 
**State** | **string** | Liveness state (&#x60;ready&#x60; | &#x60;unreachable&#x60; | &#x60;draining&#x60;). | 

## Methods

### NewWorkerSummary

`func NewWorkerSummary(apiAddr string, id int64, lastSeenUnixSecs int64, os string, state string, ) *WorkerSummary`

NewWorkerSummary instantiates a new WorkerSummary object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewWorkerSummaryWithDefaults

`func NewWorkerSummaryWithDefaults() *WorkerSummary`

NewWorkerSummaryWithDefaults instantiates a new WorkerSummary object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetApiAddr

`func (o *WorkerSummary) GetApiAddr() string`

GetApiAddr returns the ApiAddr field if non-nil, zero value otherwise.

### GetApiAddrOk

`func (o *WorkerSummary) GetApiAddrOk() (*string, bool)`

GetApiAddrOk returns a tuple with the ApiAddr field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetApiAddr

`func (o *WorkerSummary) SetApiAddr(v string)`

SetApiAddr sets ApiAddr field to given value.


### GetId

`func (o *WorkerSummary) GetId() int64`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *WorkerSummary) GetIdOk() (*int64, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *WorkerSummary) SetId(v int64)`

SetId sets Id field to given value.


### GetLabels

`func (o *WorkerSummary) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *WorkerSummary) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *WorkerSummary) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *WorkerSummary) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetLastSeenUnixSecs

`func (o *WorkerSummary) GetLastSeenUnixSecs() int64`

GetLastSeenUnixSecs returns the LastSeenUnixSecs field if non-nil, zero value otherwise.

### GetLastSeenUnixSecsOk

`func (o *WorkerSummary) GetLastSeenUnixSecsOk() (*int64, bool)`

GetLastSeenUnixSecsOk returns a tuple with the LastSeenUnixSecs field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLastSeenUnixSecs

`func (o *WorkerSummary) SetLastSeenUnixSecs(v int64)`

SetLastSeenUnixSecs sets LastSeenUnixSecs field to given value.


### GetOs

`func (o *WorkerSummary) GetOs() string`

GetOs returns the Os field if non-nil, zero value otherwise.

### GetOsOk

`func (o *WorkerSummary) GetOsOk() (*string, bool)`

GetOsOk returns a tuple with the Os field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetOs

`func (o *WorkerSummary) SetOs(v string)`

SetOs sets Os field to given value.


### GetState

`func (o *WorkerSummary) GetState() string`

GetState returns the State field if non-nil, zero value otherwise.

### GetStateOk

`func (o *WorkerSummary) GetStateOk() (*string, bool)`

GetStateOk returns a tuple with the State field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetState

`func (o *WorkerSummary) SetState(v string)`

SetState sets State field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


