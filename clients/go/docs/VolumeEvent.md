# VolumeEvent

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**ContainerId** | Pointer to **NullableString** | Container id, populated for &#x60;Mount&#x60;/&#x60;Unmount&#x60;. | [optional] 
**Driver** | Pointer to **string** | Volume driver (e.g. &#x60;local&#x60;). | [optional] 
**Kind** | [**VolumeEventKind**](VolumeEventKind.md) |  | 
**Name** | **string** | Volume name. | 

## Methods

### NewVolumeEvent

`func NewVolumeEvent(at time.Time, kind VolumeEventKind, name string, ) *VolumeEvent`

NewVolumeEvent instantiates a new VolumeEvent object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewVolumeEventWithDefaults

`func NewVolumeEventWithDefaults() *VolumeEvent`

NewVolumeEventWithDefaults instantiates a new VolumeEvent object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *VolumeEvent) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *VolumeEvent) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *VolumeEvent) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetContainerId

`func (o *VolumeEvent) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *VolumeEvent) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *VolumeEvent) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.

### HasContainerId

`func (o *VolumeEvent) HasContainerId() bool`

HasContainerId returns a boolean if a field has been set.

### SetContainerIdNil

`func (o *VolumeEvent) SetContainerIdNil(b bool)`

 SetContainerIdNil sets the value for ContainerId to be an explicit nil

### UnsetContainerId
`func (o *VolumeEvent) UnsetContainerId()`

UnsetContainerId ensures that no value is present for ContainerId, not even an explicit nil
### GetDriver

`func (o *VolumeEvent) GetDriver() string`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *VolumeEvent) GetDriverOk() (*string, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *VolumeEvent) SetDriver(v string)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *VolumeEvent) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetKind

`func (o *VolumeEvent) GetKind() VolumeEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *VolumeEvent) GetKindOk() (*VolumeEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *VolumeEvent) SetKind(v VolumeEventKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *VolumeEvent) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *VolumeEvent) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *VolumeEvent) SetName(v string)`

SetName sets Name field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


