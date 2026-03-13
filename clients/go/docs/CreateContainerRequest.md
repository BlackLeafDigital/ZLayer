# CreateContainerRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Command** | Pointer to **[]string** | Command to run (overrides image entrypoint) | [optional] 
**Env** | Pointer to **map[string]string** | Environment variables | [optional] 
**Image** | **string** | OCI image reference (e.g., \&quot;nginx:latest\&quot;, \&quot;ubuntu:22.04\&quot;) | 
**Labels** | Pointer to **map[string]string** | Labels for filtering and grouping | [optional] 
**Name** | Pointer to **NullableString** | Optional human-readable name | [optional] 
**Resources** | Pointer to [**NullableContainerResourceLimits**](ContainerResourceLimits.md) | Resource limits (CPU, memory) | [optional] 
**Volumes** | Pointer to [**[]VolumeMount**](VolumeMount.md) | Volume mounts | [optional] 
**WorkDir** | Pointer to **NullableString** | Working directory inside the container | [optional] 

## Methods

### NewCreateContainerRequest

`func NewCreateContainerRequest(image string, ) *CreateContainerRequest`

NewCreateContainerRequest instantiates a new CreateContainerRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewCreateContainerRequestWithDefaults

`func NewCreateContainerRequestWithDefaults() *CreateContainerRequest`

NewCreateContainerRequestWithDefaults instantiates a new CreateContainerRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCommand

`func (o *CreateContainerRequest) GetCommand() []string`

GetCommand returns the Command field if non-nil, zero value otherwise.

### GetCommandOk

`func (o *CreateContainerRequest) GetCommandOk() (*[]string, bool)`

GetCommandOk returns a tuple with the Command field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCommand

`func (o *CreateContainerRequest) SetCommand(v []string)`

SetCommand sets Command field to given value.

### HasCommand

`func (o *CreateContainerRequest) HasCommand() bool`

HasCommand returns a boolean if a field has been set.

### SetCommandNil

`func (o *CreateContainerRequest) SetCommandNil(b bool)`

 SetCommandNil sets the value for Command to be an explicit nil

### UnsetCommand
`func (o *CreateContainerRequest) UnsetCommand()`

UnsetCommand ensures that no value is present for Command, not even an explicit nil
### GetEnv

`func (o *CreateContainerRequest) GetEnv() map[string]string`

GetEnv returns the Env field if non-nil, zero value otherwise.

### GetEnvOk

`func (o *CreateContainerRequest) GetEnvOk() (*map[string]string, bool)`

GetEnvOk returns a tuple with the Env field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetEnv

`func (o *CreateContainerRequest) SetEnv(v map[string]string)`

SetEnv sets Env field to given value.

### HasEnv

`func (o *CreateContainerRequest) HasEnv() bool`

HasEnv returns a boolean if a field has been set.

### GetImage

`func (o *CreateContainerRequest) GetImage() string`

GetImage returns the Image field if non-nil, zero value otherwise.

### GetImageOk

`func (o *CreateContainerRequest) GetImageOk() (*string, bool)`

GetImageOk returns a tuple with the Image field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetImage

`func (o *CreateContainerRequest) SetImage(v string)`

SetImage sets Image field to given value.


### GetLabels

`func (o *CreateContainerRequest) GetLabels() map[string]string`

GetLabels returns the Labels field if non-nil, zero value otherwise.

### GetLabelsOk

`func (o *CreateContainerRequest) GetLabelsOk() (*map[string]string, bool)`

GetLabelsOk returns a tuple with the Labels field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetLabels

`func (o *CreateContainerRequest) SetLabels(v map[string]string)`

SetLabels sets Labels field to given value.

### HasLabels

`func (o *CreateContainerRequest) HasLabels() bool`

HasLabels returns a boolean if a field has been set.

### GetName

`func (o *CreateContainerRequest) GetName() string`

GetName returns the Name field if non-nil, zero value otherwise.

### GetNameOk

`func (o *CreateContainerRequest) GetNameOk() (*string, bool)`

GetNameOk returns a tuple with the Name field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetName

`func (o *CreateContainerRequest) SetName(v string)`

SetName sets Name field to given value.

### HasName

`func (o *CreateContainerRequest) HasName() bool`

HasName returns a boolean if a field has been set.

### SetNameNil

`func (o *CreateContainerRequest) SetNameNil(b bool)`

 SetNameNil sets the value for Name to be an explicit nil

### UnsetName
`func (o *CreateContainerRequest) UnsetName()`

UnsetName ensures that no value is present for Name, not even an explicit nil
### GetResources

`func (o *CreateContainerRequest) GetResources() ContainerResourceLimits`

GetResources returns the Resources field if non-nil, zero value otherwise.

### GetResourcesOk

`func (o *CreateContainerRequest) GetResourcesOk() (*ContainerResourceLimits, bool)`

GetResourcesOk returns a tuple with the Resources field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetResources

`func (o *CreateContainerRequest) SetResources(v ContainerResourceLimits)`

SetResources sets Resources field to given value.

### HasResources

`func (o *CreateContainerRequest) HasResources() bool`

HasResources returns a boolean if a field has been set.

### SetResourcesNil

`func (o *CreateContainerRequest) SetResourcesNil(b bool)`

 SetResourcesNil sets the value for Resources to be an explicit nil

### UnsetResources
`func (o *CreateContainerRequest) UnsetResources()`

UnsetResources ensures that no value is present for Resources, not even an explicit nil
### GetVolumes

`func (o *CreateContainerRequest) GetVolumes() []VolumeMount`

GetVolumes returns the Volumes field if non-nil, zero value otherwise.

### GetVolumesOk

`func (o *CreateContainerRequest) GetVolumesOk() (*[]VolumeMount, bool)`

GetVolumesOk returns a tuple with the Volumes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetVolumes

`func (o *CreateContainerRequest) SetVolumes(v []VolumeMount)`

SetVolumes sets Volumes field to given value.

### HasVolumes

`func (o *CreateContainerRequest) HasVolumes() bool`

HasVolumes returns a boolean if a field has been set.

### GetWorkDir

`func (o *CreateContainerRequest) GetWorkDir() string`

GetWorkDir returns the WorkDir field if non-nil, zero value otherwise.

### GetWorkDirOk

`func (o *CreateContainerRequest) GetWorkDirOk() (*string, bool)`

GetWorkDirOk returns a tuple with the WorkDir field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetWorkDir

`func (o *CreateContainerRequest) SetWorkDir(v string)`

SetWorkDir sets WorkDir field to given value.

### HasWorkDir

`func (o *CreateContainerRequest) HasWorkDir() bool`

HasWorkDir returns a boolean if a field has been set.

### SetWorkDirNil

`func (o *CreateContainerRequest) SetWorkDirNil(b bool)`

 SetWorkDirNil sets the value for WorkDir to be an explicit nil

### UnsetWorkDir
`func (o *CreateContainerRequest) UnsetWorkDir()`

UnsetWorkDir ensures that no value is present for WorkDir, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


