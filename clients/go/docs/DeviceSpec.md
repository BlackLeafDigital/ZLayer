# DeviceSpec

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Mknod** | Pointer to **bool** | Allow mknod (create device nodes) | [optional] 
**Path** | **string** | Host device path (e.g., /dev/kvm, /dev/net/tun) | 
**Read** | Pointer to **bool** | Allow read access | [optional] 
**Write** | Pointer to **bool** | Allow write access | [optional] 

## Methods

### NewDeviceSpec

`func NewDeviceSpec(path string, ) *DeviceSpec`

NewDeviceSpec instantiates a new DeviceSpec object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewDeviceSpecWithDefaults

`func NewDeviceSpecWithDefaults() *DeviceSpec`

NewDeviceSpecWithDefaults instantiates a new DeviceSpec object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetMknod

`func (o *DeviceSpec) GetMknod() bool`

GetMknod returns the Mknod field if non-nil, zero value otherwise.

### GetMknodOk

`func (o *DeviceSpec) GetMknodOk() (*bool, bool)`

GetMknodOk returns a tuple with the Mknod field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMknod

`func (o *DeviceSpec) SetMknod(v bool)`

SetMknod sets Mknod field to given value.

### HasMknod

`func (o *DeviceSpec) HasMknod() bool`

HasMknod returns a boolean if a field has been set.

### GetPath

`func (o *DeviceSpec) GetPath() string`

GetPath returns the Path field if non-nil, zero value otherwise.

### GetPathOk

`func (o *DeviceSpec) GetPathOk() (*string, bool)`

GetPathOk returns a tuple with the Path field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPath

`func (o *DeviceSpec) SetPath(v string)`

SetPath sets Path field to given value.


### GetRead

`func (o *DeviceSpec) GetRead() bool`

GetRead returns the Read field if non-nil, zero value otherwise.

### GetReadOk

`func (o *DeviceSpec) GetReadOk() (*bool, bool)`

GetReadOk returns a tuple with the Read field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRead

`func (o *DeviceSpec) SetRead(v bool)`

SetRead sets Read field to given value.

### HasRead

`func (o *DeviceSpec) HasRead() bool`

HasRead returns a boolean if a field has been set.

### GetWrite

`func (o *DeviceSpec) GetWrite() bool`

GetWrite returns the Write field if non-nil, zero value otherwise.

### GetWriteOk

`func (o *DeviceSpec) GetWriteOk() (*bool, bool)`

GetWriteOk returns a tuple with the Write field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWrite

`func (o *DeviceSpec) SetWrite(v bool)`

SetWrite sets Write field to given value.

### HasWrite

`func (o *DeviceSpec) HasWrite() bool`

HasWrite returns a boolean if a field has been set.


[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


