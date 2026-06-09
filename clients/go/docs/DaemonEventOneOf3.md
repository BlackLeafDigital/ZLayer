# DaemonEventOneOf3

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**At** | **time.Time** | Wall-clock time. | 
**ContainerId** | Pointer to **string** | Container id, populated for &#x60;Mount&#x60;/&#x60;Unmount&#x60;. | [optional] 
**Driver** | Pointer to **string** | Volume driver (e.g. &#x60;local&#x60;). | [optional] 
**Kind** | [**VolumeEventKind**](VolumeEventKind.md) |  | 
**Name** | **string** | Volume name. | 
**Resource** | **string** |  | 

## Methods

### NewDaemonEventOneOf3

`func NewDaemonEventOneOf3(at time.Time, kind VolumeEventKind, name string, resource string, ) *DaemonEventOneOf3`

NewDaemonEventOneOf3 instantiates a new DaemonEventOneOf3 object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDaemonEventOneOf3WithDefaults

`func NewDaemonEventOneOf3WithDefaults() *DaemonEventOneOf3`

NewDaemonEventOneOf3WithDefaults instantiates a new DaemonEventOneOf3 object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetAt

`func (o *DaemonEventOneOf3) GetAt() time.Time`

GetAt returns the At field if non-nil, zero value otherwise.

### GetAtOk

`func (o *DaemonEventOneOf3) GetAtOk() (*time.Time, bool)`

GetAtOk returns a tuple with the At field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetAt

`func (o *DaemonEventOneOf3) SetAt(v time.Time)`

SetAt sets At field to given value.


### GetContainerId

`func (o *DaemonEventOneOf3) GetContainerId() string`

GetContainerId returns the ContainerId field if non-nil, zero value otherwise.

### GetContainerIdOk

`func (o *DaemonEventOneOf3) GetContainerIdOk() (*string, bool)`

GetContainerIdOk returns a tuple with the ContainerId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetContainerId

`func (o *DaemonEventOneOf3) SetContainerId(v string)`

SetContainerId sets ContainerId field to given value.

### HasContainerId

`func (o *DaemonEventOneOf3) HasContainerId() bool`

HasContainerId returns a boolean if a field has been set.

### GetDriver

`func (o *DaemonEventOneOf3) GetDriver() string`

GetDriver returns the Driver field if non-nil, zero value otherwise.

### GetDriverOk

`func (o *DaemonEventOneOf3) GetDriverOk() (*string, bool)`

GetDriverOk returns a tuple with the Driver field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDriver

`func (o *DaemonEventOneOf3) SetDriver(v string)`

SetDriver sets Driver field to given value.

### HasDriver

`func (o *DaemonEventOneOf3) HasDriver() bool`

HasDriver returns a boolean if a field has been set.

### GetKind

`func (o *DaemonEventOneOf3) GetKind() VolumeEventKind`

GetKind returns the Kind field if non-nil, zero value otherwise.

### GetKindOk

`func (o *DaemonEventOneOf3) GetKindOk() (*VolumeEventKind, bool)`

GetKindOk returns a tuple with the Kind field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKind

`func (o *DaemonEventOneOf3) SetKind(v VolumeEventKind)`

SetKind sets Kind field to given value.


### GetName

`func (o *DaemonEventOneOf3) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *DaemonEventOneOf3) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *DaemonEventOneOf3) SetName(v string)`

SetName sets Name field to given value.


### GetResource

`func (o *DaemonEventOneOf3) GetResource() string`

GetResource returns the Resource field if non-nil, zero value otherwise.

### GetResourceOk

`func (o *DaemonEventOneOf3) GetResourceOk() (*string, bool)`

GetResourceOk returns a tuple with the Resource field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResource

`func (o *DaemonEventOneOf3) SetResource(v string)`

SetResource sets Resource field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


