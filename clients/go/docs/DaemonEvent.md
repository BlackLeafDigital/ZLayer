# DaemonEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**ExitCode** | Pointer to **int32** | Exit code, when known. Populated for &#x60;Die&#x60; events where a wait has already resolved; otherwise &#x60;None&#x60;. | [optional] 
**Id** | **string** | Network identifier (registry id). | 
**Kind** | [**VolumeEventKind**](VolumeEventKind.md) |  | 
**Labels** | Pointer to **map[string]string** | Labels on the container at the time of the event. Used by subscribers to filter via the &#x60;label&#x3D;k&#x3D;v&#x60; query param (AND semantics). | [optional] 
**Reason** | Pointer to **string** | Free-form human-readable reason. For &#x60;Die&#x60;, may indicate \&quot;stopped\&quot;, \&quot;killed\&quot;, \&quot;oom-killed\&quot;; for &#x60;Oom&#x60;, the OOM detail; for &#x60;Health&#x60;, may echo the probe failure message. | [optional] 
**Status** | Pointer to **string** | Health status, only populated for &#x60;Health&#x60; events (e.g. \&quot;healthy\&quot;, \&quot;unhealthy\&quot;, \&quot;starting\&quot;). | [optional] 
**Resource** | **string** |  | 
**Digest** | Pointer to **string** | Optional content digest (&#x60;sha256:...&#x60;) when known. | [optional] 
**Reference** | **string** | Image reference (e.g. &#x60;nginx:latest&#x60; or &#x60;registry/foo/bar@sha256:...&#x60;). | 
**Source** | Pointer to **string** | Source reference for &#x60;Tag&#x60; events (the image being aliased). | [optional] 
**ContainerId** | Pointer to **string** | Container id, populated for &#x60;Mount&#x60;/&#x60;Unmount&#x60;. | [optional] 
**Driver** | Pointer to **string** | Volume driver (e.g. &#x60;local&#x60;). | [optional] 
**Name** | **string** | Volume name. | 

## Methods

### NewDaemonEvent

`func NewDaemonEvent(at time.Time, id string, kind VolumeEventKind, resource string, reference string, name string, ) *DaemonEvent`

NewDaemonEvent instantiates a new DaemonEvent object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonEventWithDefaults

`func NewDaemonEventWithDefaults() *DaemonEvent`

NewDaemonEventWithDefaults instantiates a new DaemonEvent object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *DaemonEvent) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *DaemonEvent) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *DaemonEvent) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetExitCode

`func (o *DaemonEvent) GetExitCode() int32`

GetExitCode returns the ExitCode field if non-nil, zero value otherwise.

### GetExitCodeOk

`func (o *DaemonEvent) GetExitCodeOk() (*int32, bool)`

GetExitCodeOk returns a tuple with the ExitCode field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetExitCode

`func (o *DaemonEvent) SetExitCode(v int32)`

SetExitCode sets ExitCode field to given value.

### HasExitCode

`func (o *DaemonEvent) HasExitCode() bool`

HasExitCode returns a boolean if a field has been set.

### GetId

`func (o *DaemonEvent) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *DaemonEvent) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *DaemonEvent) SetId(v string)`

SetId sets Id field to given value.


### GetKind

`func (o *DaemonEvent) GetKind() VolumeEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *DaemonEvent) GetKindOk() (*VolumeEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *DaemonEvent) SetKind(v VolumeEventKind)`

SetKind sets Kind field to given value.


### GetLabels

`func (o *DaemonEvent) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *DaemonEvent) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *DaemonEvent) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *DaemonEvent) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetReason

`func (o *DaemonEvent) GetReason() string`

GetReason returns the Reason field if non-nil, zero value otherwise.

### GetReasonOk

`func (o *DaemonEvent) GetReasonOk() (*string, bool)`

GetReasonOk returns a tuple with the Reason field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReason

`func (o *DaemonEvent) SetReason(v string)`

SetReason sets Reason field to given value.

### HasReason

`func (o *DaemonEvent) HasReason() bool`

HasReason returns a boolean if a field has been set.

### GetStatus

`func (o *DaemonEvent) GetStatus() string`

GetStatus returns the Status field if non-nil, zero value otherwise.

### GetStatusOk

`func (o *DaemonEvent) GetStatusOk() (*string, bool)`

GetStatusOk returns a tuple with the Status field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetStatus

`func (o *DaemonEvent) SetStatus(v string)`

SetStatus sets Status field to given value.

### HasStatus

`func (o *DaemonEvent) HasStatus() bool`

HasStatus returns a boolean if a field has been set.

### GetResource

`func (o *DaemonEvent) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *DaemonEvent) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *DaemonEvent) SetResource(v string)`

SetResource sets Resource field to given value.


### GetDigest

`func (o *DaemonEvent) GetDigest() string`

GetDigest returns the Digest field if non-nil, zero value otherwise.

### GetDigestOk

`func (o *DaemonEvent) GetDigestOk() (*string, bool)`

GetDigestOk returns a tuple with the Digest field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDigest

`func (o *DaemonEvent) SetDigest(v string)`

SetDigest sets Digest field to given value.

### HasDigest

`func (o *DaemonEvent) HasDigest() bool`

HasDigest returns a boolean if a field has been set.

### GetReference

`func (o *DaemonEvent) GetReference() string`

GetReference returns the Reference field if non-nil, zero value otherwise.

### GetReferenceOk

`func (o *DaemonEvent) GetReferenceOk() (*string, bool)`

GetReferenceOk returns a tuple with the Reference field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetReference

`func (o *DaemonEvent) SetReference(v string)`

SetReference sets Reference field to given value.


### GetSource

`func (o *DaemonEvent) GetSource() string`

GetSource returns the Source field if non-nil, zero value otherwise.

### GetSourceOk

`func (o *DaemonEvent) GetSourceOk() (*string, bool)`

GetSourceOk returns a tuple with the Source field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetSource

`func (o *DaemonEvent) SetSource(v string)`

SetSource sets Source field to given value.

### HasSource

`func (o *DaemonEvent) HasSource() bool`

HasSource returns a boolean if a field has been set.

### GetContainerId

`func (o *DaemonEvent) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *DaemonEvent) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *DaemonEvent) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.

### HasContainerId

`func (o *DaemonEvent) HasContainerId() bool`

HasContainerId returns a boolean if a field has been set.

### GetDriver

`func (o *DaemonEvent) GetDriver() string`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *DaemonEvent) GetDriverOk() (*string, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *DaemonEvent) SetDriver(v string)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *DaemonEvent) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetName

`func (o *DaemonEvent) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *DaemonEvent) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *DaemonEvent) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


