# ContainerResourceLimits

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Cpu** | Pointer to **NullableFloat64** | CPU limit in cores (e.g., 0.5, 1.0, 2.0) | [optional] 
**Memory** | Pointer to **NullableString** | Memory limit (e.g., \&quot;256Mi\&quot;, \&quot;1Gi\&quot;) | [optional] 

## Methods

### NewContainerResourceLimits

`func NewContainerResourceLimits() *ContainerResourceLimits`

NewContainerResourceLimits instantiates a new ContainerResourceLimits object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerResourceLimitsWithDefaults

`func NewContainerResourceLimitsWithDefaults() *ContainerResourceLimits`

NewContainerResourceLimitsWithDefaults instantiates a new ContainerResourceLimits object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCpu

`func (o *ContainerResourceLimits) GetCpu() float64`

GetCpu returns the Cpu field if non-nil, zero value otherwise.

### GetCpuOk

`func (o *ContainerResourceLimits) GetCpuOk() (*float64, bool)`

GetCpuOk returns a tuple with the Cpu field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpu

`func (o *ContainerResourceLimits) SetCpu(v float64)`

SetCpu sets Cpu field to given value.

### HasCpu

`func (o *ContainerResourceLimits) HasCpu() bool`

HasCpu returns a boolean if a field has been set.

### SetCpuNil

`func (o *ContainerResourceLimits) SetCpuNil(b bool)`

 SetCpuNil sets the value for Cpu to be an explicit nil

### UnsetCpu
`func (o *ContainerResourceLimits) UnsetCpu()`

UnsetCpu ensures that no value is present for Cpu, not even an explicit nil
### GetMemory

`func (o *ContainerResourceLimits) GetMemory() string`

GetMemory returns the Memory field if non-nil, zero value otherwise.

### GetMemoryOk

`func (o *ContainerResourceLimits) GetMemoryOk() (*string, bool)`

GetMemoryOk returns a tuple with the Memory field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemory

`func (o *ContainerResourceLimits) SetMemory(v string)`

SetMemory sets Memory field to given value.

### HasMemory

`func (o *ContainerResourceLimits) HasMemory() bool`

HasMemory returns a boolean if a field has been set.

### SetMemoryNil

`func (o *ContainerResourceLimits) SetMemoryNil(b bool)`

 SetMemoryNil sets the value for Memory to be an explicit nil

### UnsetMemory
`func (o *ContainerResourceLimits) UnsetMemory()`

UnsetMemory ensures that no value is present for Memory, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


