# ContainerEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time the event was emitted.  Serialized as an RFC 3339 string on the wire. Typed as [&#x60;chrono::DateTime&lt;Utc&gt;&#x60;] in-process for ergonomic manipulation; the &#x60;#[schema(value_type &#x3D; String, format &#x3D; DateTime)]&#x60; attribute teaches &#x60;utoipa&#x60; to render a proper string schema in the &#x60;OpenAPI&#x60; output. | 
**ExitCode** | Pointer to **NullableInt32** | Exit code, when known. Populated for &#x60;Die&#x60; events where a wait has already resolved; otherwise &#x60;None&#x60;. | [optional] 
**Id** | **string** | Container identifier (the API&#39;s id string, not the raw runtime id). | 
**Kind** | [**ContainerEventKind**](ContainerEventKind.md) | What kind of transition this event represents. | 
**Labels** | Pointer to **map[string]string** | Labels on the container at the time of the event. Used by subscribers to filter via the &#x60;label&#x3D;k&#x3D;v&#x60; query param (AND semantics). | [optional] 
**Reason** | Pointer to **NullableString** | Free-form human-readable reason. For &#x60;Die&#x60;, may indicate \&quot;stopped\&quot;, \&quot;killed\&quot;, \&quot;oom-killed\&quot;; for &#x60;Oom&#x60;, the OOM detail; for &#x60;Health&#x60;, may echo the probe failure message. | [optional] 
**Status** | Pointer to **NullableString** | Health status, only populated for &#x60;Health&#x60; events (e.g. \&quot;healthy\&quot;, \&quot;unhealthy\&quot;, \&quot;starting\&quot;). | [optional] 

## Methods

### NewContainerEvent

`func NewContainerEvent(at time.Time, id string, kind ContainerEventKind, ) *ContainerEvent`

NewContainerEvent instantiates a new ContainerEvent object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerEventWithDefaults

`func NewContainerEventWithDefaults() *ContainerEvent`

NewContainerEventWithDefaults instantiates a new ContainerEvent object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *ContainerEvent) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *ContainerEvent) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *ContainerEvent) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetExitCode

`func (o *ContainerEvent) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *ContainerEvent) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *ContainerEvent) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.

### HasExitCode

`func (o *ContainerEvent) HasExitCode() bool`

HasExitCode returns a boolean if a field has been set.

### SetExitCodeNil

`func (o *ContainerEvent) SetExitCodeNil(b bool)`

 SetExitCodeNil sets the value for ExitCode to be an explicit nil

### UnsetExitCode
`func (o *ContainerEvent) UnsetExitCode()`

UnsetExitCode ensures that no value is present for ExitCode, not even an explicit nil
### GetId

`func (o *ContainerEvent) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerEvent) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerEvent) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *ContainerEvent) GetKind() ContainerEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *ContainerEvent) GetKindOk() (*ContainerEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *ContainerEvent) SetKind(v ContainerEventKind)`

SetKind sets Kind field to given value.


### GetLabels

`func (o *ContainerEvent) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *ContainerEvent) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *ContainerEvent) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *ContainerEvent) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetReason

`func (o *ContainerEvent) GetReason() string`

GetReason returns the Reason field if non-nil, zero value otherwise.

### GetReasonOk

`func (o *ContainerEvent) GetReasonOk() (*string, bool)`

GetReasonOk returns a tuple with the Reason field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReason

`func (o *ContainerEvent) SetReason(v string)`

SetReason sets Reason field to given value.

### HasReason

`func (o *ContainerEvent) HasReason() bool`

HasReason returns a boolean if a field has been set.

### SetReasonNil

`func (o *ContainerEvent) SetReasonNil(b bool)`

 SetReasonNil sets the value for Reason to be an explicit nil

### UnsetReason
`func (o *ContainerEvent) UnsetReason()`

UnsetReason ensures that no value is present for Reason, not even an explicit nil
### GetStatus

`func (o *ContainerEvent) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *ContainerEvent) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *ContainerEvent) SetStatus(v string)`

SetStatus sets Status field to given value.

### HasStatus

`func (o *ContainerEvent) HasStatus() bool`

HasStatus returns a boolean if a field has been set.

### SetStatusNil

`func (o *ContainerEvent) SetStatusNil(b bool)`

 SetStatusNil sets the value for Status to be an explicit nil

### UnsetStatus
`func (o *ContainerEvent) UnsetStatus()`

UnsetStatus ensures that no value is present for Status, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


