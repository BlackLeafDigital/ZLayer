# DaemonEventOneOf

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time the event was emitted.  Serialized as an RFC 3339 string on the wire. | 
**ExitCode** | Pointer to **int32** | Exit code, when known. Populated for &#x60;Die&#x60; events where a wait has already resolved; otherwise &#x60;None&#x60;. | [optional] 
**Id** | **string** | Container identifier (the API&#39;s id string, not the raw runtime id). | 
**Kind** | [**ContainerEventKind**](ContainerEventKind.md) | What kind of transition this event represents. | 
**Labels** | Pointer to **map[string]string** | Labels on the container at the time of the event. Used by subscribers to filter via the &#x60;label&#x3D;k&#x3D;v&#x60; query param (AND semantics). | [optional] 
**Reason** | Pointer to **string** | Free-form human-readable reason. For &#x60;Die&#x60;, may indicate \&quot;stopped\&quot;, \&quot;killed\&quot;, \&quot;oom-killed\&quot;; for &#x60;Oom&#x60;, the OOM detail; for &#x60;Health&#x60;, may echo the probe failure message. | [optional] 
**Status** | Pointer to **string** | Health status, only populated for &#x60;Health&#x60; events (e.g. \&quot;healthy\&quot;, \&quot;unhealthy\&quot;, \&quot;starting\&quot;). | [optional] 
**Resource** | **string** |  | 

## Methods

### NewDaemonEventOneOf

`func NewDaemonEventOneOf(at time.Time, id string, kind ContainerEventKind, resource string, ) *DaemonEventOneOf`

NewDaemonEventOneOf instantiates a new DaemonEventOneOf object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonEventOneOfWithDefaults

`func NewDaemonEventOneOfWithDefaults() *DaemonEventOneOf`

NewDaemonEventOneOfWithDefaults instantiates a new DaemonEventOneOf object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *DaemonEventOneOf) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *DaemonEventOneOf) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *DaemonEventOneOf) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetExitCode

`func (o *DaemonEventOneOf) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *DaemonEventOneOf) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *DaemonEventOneOf) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.

### HasExitCode

`func (o *DaemonEventOneOf) HasExitCode() bool`

HasExitCode returns a boolean if a field has been set.

### GetId

`func (o *DaemonEventOneOf) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *DaemonEventOneOf) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *DaemonEventOneOf) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *DaemonEventOneOf) GetKind() ContainerEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *DaemonEventOneOf) GetKindOk() (*ContainerEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *DaemonEventOneOf) SetKind(v ContainerEventKind)`

SetKind sets Kind field to given value.


### GetLabels

`func (o *DaemonEventOneOf) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *DaemonEventOneOf) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *DaemonEventOneOf) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *DaemonEventOneOf) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetReason

`func (o *DaemonEventOneOf) GetReason() string`

GetReason returns the Reason field if non-nil, zero value otherwise.

### GetReasonOk

`func (o *DaemonEventOneOf) GetReasonOk() (*string, bool)`

GetReasonOk returns a tuple with the Reason field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReason

`func (o *DaemonEventOneOf) SetReason(v string)`

SetReason sets Reason field to given value.

### HasReason

`func (o *DaemonEventOneOf) HasReason() bool`

HasReason returns a boolean if a field has been set.

### GetStatus

`func (o *DaemonEventOneOf) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *DaemonEventOneOf) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *DaemonEventOneOf) SetStatus(v string)`

SetStatus sets Status field to given value.

### HasStatus

`func (o *DaemonEventOneOf) HasStatus() bool`

HasStatus returns a boolean if a field has been set.

### GetResource

`func (o *DaemonEventOneOf) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *DaemonEventOneOf) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *DaemonEventOneOf) SetResource(v string)`

SetResource sets Resource field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


