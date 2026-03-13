# ContainerInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CreatedAt** | **string** | Creation timestamp (ISO 8601) | 
**Id** | **string** | Container identifier | 
**Image** | **string** | OCI image reference | 
**Labels** | **map[string]string** | Labels | 
**Name** | Pointer to **NullableString** | Human-readable name (if set) | [optional] 
**Pid** | Pointer to **NullableInt32** | Process ID (if running) | [optional] 
**State** | **string** | Container state (pending, running, exited, failed) | 

## Methods

### NewContainerInfo

`func NewContainerInfo(createdAt string, id string, image string, labels map[string]string, state string, ) *ContainerInfo`

NewContainerInfo instantiates a new ContainerInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerInfoWithDefaults

`func NewContainerInfoWithDefaults() *ContainerInfo`

NewContainerInfoWithDefaults instantiates a new ContainerInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCreatedAt

`func (o *ContainerInfo) GetCreatedAt() string`

GetCreatedAt returns the CreatedAt field if non-nil, zero value otherwise.

### GetCreatedAtOk

`func (o *ContainerInfo) GetCreatedAtOk() (*string, bool)`

GetCreatedAtOk returns a tuple with the CreatedAt field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCreatedAt

`func (o *ContainerInfo) SetCreatedAt(v string)`

SetCreatedAt sets CreatedAt field to given value.


### GetId

`func (o *ContainerInfo) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerInfo) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerInfo) SetId(v string)`

SetId sets Id field to given value.


### GetImage

`func (o *ContainerInfo) GetImage() string`

GetImage returns the Image field if non-nil, zero value otherwise.

### GetImageOk

`func (o *ContainerInfo) GetImageOk() (*string, bool)`

GetImageOk returns a tuple with the Image field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetImage

`func (o *ContainerInfo) SetImage(v string)`

SetImage sets Image field to given value.


### GetLabels

`func (o *ContainerInfo) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *ContainerInfo) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *ContainerInfo) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.


### GetName

`func (o *ContainerInfo) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *ContainerInfo) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *ContainerInfo) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *ContainerInfo) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *ContainerInfo) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *ContainerInfo) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil
### GetPid

`func (o *ContainerInfo) GetPid() int32`

GetPid returns the Pid field if non-nil, zero value otherwise.

### GetPidOk

`func (o *ContainerInfo) GetPidOk() (*int32, bool)`

GetPidOk returns a tuple with the Pid field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPid

`func (o *ContainerInfo) SetPid(v int32)`

SetPid sets Pid field to given value.

### HasPid

`func (o *ContainerInfo) HasPid() bool`

HasPid returns a boolean if a field has been set.

### SetPidNil

`func (o *ContainerInfo) SetPidNil(b bool)`

 SetPidNil sets the value for Pid to be an explicit nil

### UnsetPid
`func (o *ContainerInfo) UnsetPid()`

UnsetPid ensures that no value is present for Pid, not even an explicit nil
### GetState

`func (o *ContainerInfo) GetState() string`

GetState returns the State field if non-nil, zero value otherwise.

### GetStateOk

`func (o *ContainerInfo) GetStateOk() (*string, bool)`

GetStateOk returns a tuple with the State field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetState

`func (o *ContainerInfo) SetState(v string)`

SetState sets State field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


