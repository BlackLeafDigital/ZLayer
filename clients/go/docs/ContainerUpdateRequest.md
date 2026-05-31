# ContainerUpdateRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**BlkioWeight** | Pointer to **NullableInt32** | Block IO weight (relative weight, range 10-1000). | [optional] 
**CpuPeriod** | Pointer to **NullableInt64** | CPU CFS period in microseconds. | [optional] 
**CpuQuota** | Pointer to **NullableInt64** | CPU CFS quota in microseconds. Together with &#x60;cpu_period&#x60; defines the fraction of a CPU the container may use. | [optional] 
**CpuRealtimePeriod** | Pointer to **NullableInt64** | CPU real-time period in microseconds. | [optional] 
**CpuRealtimeRuntime** | Pointer to **NullableInt64** | CPU real-time runtime in microseconds. | [optional] 
**CpuShares** | Pointer to **NullableInt64** | Relative CPU weight (cgroup &#x60;cpu.weight&#x60; or &#x60;cpu.shares&#x60;). Range 2-262144 on cgroup v2; 2-262144 mapped from 1-10000 on v1. | [optional] 
**CpusetCpus** | Pointer to **NullableString** | CPUs allowed for execution (e.g. &#x60;\&quot;0-3\&quot;&#x60;, &#x60;\&quot;0,1\&quot;&#x60;). | [optional] 
**CpusetMems** | Pointer to **NullableString** | Memory nodes (NUMA) allowed for execution (e.g. &#x60;\&quot;0-3\&quot;&#x60;). | [optional] 
**KernelMemory** | Pointer to **NullableInt64** | Kernel memory limit in bytes (deprecated upstream; accepted for wire compatibility). | [optional] 
**Memory** | Pointer to **NullableInt64** | Memory limit in bytes. Set &#x60;0&#x60; to remove the limit. | [optional] 
**MemoryReservation** | Pointer to **NullableInt64** | Soft memory limit in bytes. The kernel reclaims pages above this reservation when the host comes under memory pressure. | [optional] 
**MemorySwap** | Pointer to **NullableInt64** | Total memory limit (memory + swap) in bytes. &#x60;-1&#x60; removes the swap limit, matching Docker semantics. | [optional] 
**PidsLimit** | Pointer to **NullableInt64** | PIDs limit. Set &#x60;0&#x60; or &#x60;-1&#x60; for unlimited. | [optional] 
**RestartPolicy** | Pointer to [**NullableContainerUpdateRestartPolicy**](ContainerUpdateRestartPolicy.md) | New restart policy. When present, replaces the container&#39;s stored restart policy. Docker applies this asynchronously: the next time the supervisor decides whether to restart, it consults the new policy. | [optional] 

## Methods

### NewContainerUpdateRequest

`func NewContainerUpdateRequest() *ContainerUpdateRequest`

NewContainerUpdateRequest instantiates a new ContainerUpdateRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerUpdateRequestWithDefaults

`func NewContainerUpdateRequestWithDefaults() *ContainerUpdateRequest`

NewContainerUpdateRequestWithDefaults instantiates a new ContainerUpdateRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetBlkioWeight

`func (o *ContainerUpdateRequest) GetBlkioWeight() int32`

GetBlkioWeight returns the BlkioWeight field if non-nil, zero value otherwise.

### GetBlkioWeightOk

`func (o *ContainerUpdateRequest) GetBlkioWeightOk() (*int32, bool)`

GetBlkioWeightOk returns a tuple with the BlkioWeight field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetBlkioWeight

`func (o *ContainerUpdateRequest) SetBlkioWeight(v int32)`

SetBlkioWeight sets BlkioWeight field to given value.

### HasBlkioWeight

`func (o *ContainerUpdateRequest) HasBlkioWeight() bool`

HasBlkioWeight returns a boolean if a field has been set.

### SetBlkioWeightNil

`func (o *ContainerUpdateRequest) SetBlkioWeightNil(b bool)`

 SetBlkioWeightNil sets the value for BlkioWeight to be an explicit nil

### UnsetBlkioWeight
`func (o *ContainerUpdateRequest) UnsetBlkioWeight()`

UnsetBlkioWeight ensures that no value is present for BlkioWeight, not even an explicit nil
### GetCpuPeriod

`func (o *ContainerUpdateRequest) GetCpuPeriod() int64`

GetCpuPeriod returns the CpuPeriod field if non-nil, zero value otherwise.

### GetCpuPeriodOk

`func (o *ContainerUpdateRequest) GetCpuPeriodOk() (*int64, bool)`

GetCpuPeriodOk returns a tuple with the CpuPeriod field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuPeriod

`func (o *ContainerUpdateRequest) SetCpuPeriod(v int64)`

SetCpuPeriod sets CpuPeriod field to given value.

### HasCpuPeriod

`func (o *ContainerUpdateRequest) HasCpuPeriod() bool`

HasCpuPeriod returns a boolean if a field has been set.

### SetCpuPeriodNil

`func (o *ContainerUpdateRequest) SetCpuPeriodNil(b bool)`

 SetCpuPeriodNil sets the value for CpuPeriod to be an explicit nil

### UnsetCpuPeriod
`func (o *ContainerUpdateRequest) UnsetCpuPeriod()`

UnsetCpuPeriod ensures that no value is present for CpuPeriod, not even an explicit nil
### GetCpuQuota

`func (o *ContainerUpdateRequest) GetCpuQuota() int64`

GetCpuQuota returns the CpuQuota field if non-nil, zero value otherwise.

### GetCpuQuotaOk

`func (o *ContainerUpdateRequest) GetCpuQuotaOk() (*int64, bool)`

GetCpuQuotaOk returns a tuple with the CpuQuota field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuQuota

`func (o *ContainerUpdateRequest) SetCpuQuota(v int64)`

SetCpuQuota sets CpuQuota field to given value.

### HasCpuQuota

`func (o *ContainerUpdateRequest) HasCpuQuota() bool`

HasCpuQuota returns a boolean if a field has been set.

### SetCpuQuotaNil

`func (o *ContainerUpdateRequest) SetCpuQuotaNil(b bool)`

 SetCpuQuotaNil sets the value for CpuQuota to be an explicit nil

### UnsetCpuQuota
`func (o *ContainerUpdateRequest) UnsetCpuQuota()`

UnsetCpuQuota ensures that no value is present for CpuQuota, not even an explicit nil
### GetCpuRealtimePeriod

`func (o *ContainerUpdateRequest) GetCpuRealtimePeriod() int64`

GetCpuRealtimePeriod returns the CpuRealtimePeriod field if non-nil, zero value otherwise.

### GetCpuRealtimePeriodOk

`func (o *ContainerUpdateRequest) GetCpuRealtimePeriodOk() (*int64, bool)`

GetCpuRealtimePeriodOk returns a tuple with the CpuRealtimePeriod field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuRealtimePeriod

`func (o *ContainerUpdateRequest) SetCpuRealtimePeriod(v int64)`

SetCpuRealtimePeriod sets CpuRealtimePeriod field to given value.

### HasCpuRealtimePeriod

`func (o *ContainerUpdateRequest) HasCpuRealtimePeriod() bool`

HasCpuRealtimePeriod returns a boolean if a field has been set.

### SetCpuRealtimePeriodNil

`func (o *ContainerUpdateRequest) SetCpuRealtimePeriodNil(b bool)`

 SetCpuRealtimePeriodNil sets the value for CpuRealtimePeriod to be an explicit nil

### UnsetCpuRealtimePeriod
`func (o *ContainerUpdateRequest) UnsetCpuRealtimePeriod()`

UnsetCpuRealtimePeriod ensures that no value is present for CpuRealtimePeriod, not even an explicit nil
### GetCpuRealtimeRuntime

`func (o *ContainerUpdateRequest) GetCpuRealtimeRuntime() int64`

GetCpuRealtimeRuntime returns the CpuRealtimeRuntime field if non-nil, zero value otherwise.

### GetCpuRealtimeRuntimeOk

`func (o *ContainerUpdateRequest) GetCpuRealtimeRuntimeOk() (*int64, bool)`

GetCpuRealtimeRuntimeOk returns a tuple with the CpuRealtimeRuntime field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuRealtimeRuntime

`func (o *ContainerUpdateRequest) SetCpuRealtimeRuntime(v int64)`

SetCpuRealtimeRuntime sets CpuRealtimeRuntime field to given value.

### HasCpuRealtimeRuntime

`func (o *ContainerUpdateRequest) HasCpuRealtimeRuntime() bool`

HasCpuRealtimeRuntime returns a boolean if a field has been set.

### SetCpuRealtimeRuntimeNil

`func (o *ContainerUpdateRequest) SetCpuRealtimeRuntimeNil(b bool)`

 SetCpuRealtimeRuntimeNil sets the value for CpuRealtimeRuntime to be an explicit nil

### UnsetCpuRealtimeRuntime
`func (o *ContainerUpdateRequest) UnsetCpuRealtimeRuntime()`

UnsetCpuRealtimeRuntime ensures that no value is present for CpuRealtimeRuntime, not even an explicit nil
### GetCpuShares

`func (o *ContainerUpdateRequest) GetCpuShares() int64`

GetCpuShares returns the CpuShares field if non-nil, zero value otherwise.

### GetCpuSharesOk

`func (o *ContainerUpdateRequest) GetCpuSharesOk() (*int64, bool)`

GetCpuSharesOk returns a tuple with the CpuShares field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuShares

`func (o *ContainerUpdateRequest) SetCpuShares(v int64)`

SetCpuShares sets CpuShares field to given value.

### HasCpuShares

`func (o *ContainerUpdateRequest) HasCpuShares() bool`

HasCpuShares returns a boolean if a field has been set.

### SetCpuSharesNil

`func (o *ContainerUpdateRequest) SetCpuSharesNil(b bool)`

 SetCpuSharesNil sets the value for CpuShares to be an explicit nil

### UnsetCpuShares
`func (o *ContainerUpdateRequest) UnsetCpuShares()`

UnsetCpuShares ensures that no value is present for CpuShares, not even an explicit nil
### GetCpusetCpus

`func (o *ContainerUpdateRequest) GetCpusetCpus() string`

GetCpusetCpus returns the CpusetCpus field if non-nil, zero value otherwise.

### GetCpusetCpusOk

`func (o *ContainerUpdateRequest) GetCpusetCpusOk() (*string, bool)`

GetCpusetCpusOk returns a tuple with the CpusetCpus field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpusetCpus

`func (o *ContainerUpdateRequest) SetCpusetCpus(v string)`

SetCpusetCpus sets CpusetCpus field to given value.

### HasCpusetCpus

`func (o *ContainerUpdateRequest) HasCpusetCpus() bool`

HasCpusetCpus returns a boolean if a field has been set.

### SetCpusetCpusNil

`func (o *ContainerUpdateRequest) SetCpusetCpusNil(b bool)`

 SetCpusetCpusNil sets the value for CpusetCpus to be an explicit nil

### UnsetCpusetCpus
`func (o *ContainerUpdateRequest) UnsetCpusetCpus()`

UnsetCpusetCpus ensures that no value is present for CpusetCpus, not even an explicit nil
### GetCpusetMems

`func (o *ContainerUpdateRequest) GetCpusetMems() string`

GetCpusetMems returns the CpusetMems field if non-nil, zero value otherwise.

### GetCpusetMemsOk

`func (o *ContainerUpdateRequest) GetCpusetMemsOk() (*string, bool)`

GetCpusetMemsOk returns a tuple with the CpusetMems field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpusetMems

`func (o *ContainerUpdateRequest) SetCpusetMems(v string)`

SetCpusetMems sets CpusetMems field to given value.

### HasCpusetMems

`func (o *ContainerUpdateRequest) HasCpusetMems() bool`

HasCpusetMems returns a boolean if a field has been set.

### SetCpusetMemsNil

`func (o *ContainerUpdateRequest) SetCpusetMemsNil(b bool)`

 SetCpusetMemsNil sets the value for CpusetMems to be an explicit nil

### UnsetCpusetMems
`func (o *ContainerUpdateRequest) UnsetCpusetMems()`

UnsetCpusetMems ensures that no value is present for CpusetMems, not even an explicit nil
### GetKernelMemory

`func (o *ContainerUpdateRequest) GetKernelMemory() int64`

GetKernelMemory returns the KernelMemory field if non-nil, zero value otherwise.

### GetKernelMemoryOk

`func (o *ContainerUpdateRequest) GetKernelMemoryOk() (*int64, bool)`

GetKernelMemoryOk returns a tuple with the KernelMemory field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetKernelMemory

`func (o *ContainerUpdateRequest) SetKernelMemory(v int64)`

SetKernelMemory sets KernelMemory field to given value.

### HasKernelMemory

`func (o *ContainerUpdateRequest) HasKernelMemory() bool`

HasKernelMemory returns a boolean if a field has been set.

### SetKernelMemoryNil

`func (o *ContainerUpdateRequest) SetKernelMemoryNil(b bool)`

 SetKernelMemoryNil sets the value for KernelMemory to be an explicit nil

### UnsetKernelMemory
`func (o *ContainerUpdateRequest) UnsetKernelMemory()`

UnsetKernelMemory ensures that no value is present for KernelMemory, not even an explicit nil
### GetMemory

`func (o *ContainerUpdateRequest) GetMemory() int64`

GetMemory returns the Memory field if non-nil, zero value otherwise.

### GetMemoryOk

`func (o *ContainerUpdateRequest) GetMemoryOk() (*int64, bool)`

GetMemoryOk returns a tuple with the Memory field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemory

`func (o *ContainerUpdateRequest) SetMemory(v int64)`

SetMemory sets Memory field to given value.

### HasMemory

`func (o *ContainerUpdateRequest) HasMemory() bool`

HasMemory returns a boolean if a field has been set.

### SetMemoryNil

`func (o *ContainerUpdateRequest) SetMemoryNil(b bool)`

 SetMemoryNil sets the value for Memory to be an explicit nil

### UnsetMemory
`func (o *ContainerUpdateRequest) UnsetMemory()`

UnsetMemory ensures that no value is present for Memory, not even an explicit nil
### GetMemoryReservation

`func (o *ContainerUpdateRequest) GetMemoryReservation() int64`

GetMemoryReservation returns the MemoryReservation field if non-nil, zero value otherwise.

### GetMemoryReservationOk

`func (o *ContainerUpdateRequest) GetMemoryReservationOk() (*int64, bool)`

GetMemoryReservationOk returns a tuple with the MemoryReservation field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryReservation

`func (o *ContainerUpdateRequest) SetMemoryReservation(v int64)`

SetMemoryReservation sets MemoryReservation field to given value.

### HasMemoryReservation

`func (o *ContainerUpdateRequest) HasMemoryReservation() bool`

HasMemoryReservation returns a boolean if a field has been set.

### SetMemoryReservationNil

`func (o *ContainerUpdateRequest) SetMemoryReservationNil(b bool)`

 SetMemoryReservationNil sets the value for MemoryReservation to be an explicit nil

### UnsetMemoryReservation
`func (o *ContainerUpdateRequest) UnsetMemoryReservation()`

UnsetMemoryReservation ensures that no value is present for MemoryReservation, not even an explicit nil
### GetMemorySwap

`func (o *ContainerUpdateRequest) GetMemorySwap() int64`

GetMemorySwap returns the MemorySwap field if non-nil, zero value otherwise.

### GetMemorySwapOk

`func (o *ContainerUpdateRequest) GetMemorySwapOk() (*int64, bool)`

GetMemorySwapOk returns a tuple with the MemorySwap field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemorySwap

`func (o *ContainerUpdateRequest) SetMemorySwap(v int64)`

SetMemorySwap sets MemorySwap field to given value.

### HasMemorySwap

`func (o *ContainerUpdateRequest) HasMemorySwap() bool`

HasMemorySwap returns a boolean if a field has been set.

### SetMemorySwapNil

`func (o *ContainerUpdateRequest) SetMemorySwapNil(b bool)`

 SetMemorySwapNil sets the value for MemorySwap to be an explicit nil

### UnsetMemorySwap
`func (o *ContainerUpdateRequest) UnsetMemorySwap()`

UnsetMemorySwap ensures that no value is present for MemorySwap, not even an explicit nil
### GetPidsLimit

`func (o *ContainerUpdateRequest) GetPidsLimit() int64`

GetPidsLimit returns the PidsLimit field if non-nil, zero value otherwise.

### GetPidsLimitOk

`func (o *ContainerUpdateRequest) GetPidsLimitOk() (*int64, bool)`

GetPidsLimitOk returns a tuple with the PidsLimit field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPidsLimit

`func (o *ContainerUpdateRequest) SetPidsLimit(v int64)`

SetPidsLimit sets PidsLimit field to given value.

### HasPidsLimit

`func (o *ContainerUpdateRequest) HasPidsLimit() bool`

HasPidsLimit returns a boolean if a field has been set.

### SetPidsLimitNil

`func (o *ContainerUpdateRequest) SetPidsLimitNil(b bool)`

 SetPidsLimitNil sets the value for PidsLimit to be an explicit nil

### UnsetPidsLimit
`func (o *ContainerUpdateRequest) UnsetPidsLimit()`

UnsetPidsLimit ensures that no value is present for PidsLimit, not even an explicit nil
### GetRestartPolicy

`func (o *ContainerUpdateRequest) GetRestartPolicy() ContainerUpdateRestartPolicy`

GetRestartPolicy returns the RestartPolicy field if non-nil, zero value otherwise.

### GetRestartPolicyOk

`func (o *ContainerUpdateRequest) GetRestartPolicyOk() (*ContainerUpdateRestartPolicy, bool)`

GetRestartPolicyOk returns a tuple with the RestartPolicy field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRestartPolicy

`func (o *ContainerUpdateRequest) SetRestartPolicy(v ContainerUpdateRestartPolicy)`

SetRestartPolicy sets RestartPolicy field to given value.

### HasRestartPolicy

`func (o *ContainerUpdateRequest) HasRestartPolicy() bool`

HasRestartPolicy returns a boolean if a field has been set.

### SetRestartPolicyNil

`func (o *ContainerUpdateRequest) SetRestartPolicyNil(b bool)`

 SetRestartPolicyNil sets the value for RestartPolicy to be an explicit nil

### UnsetRestartPolicy
`func (o *ContainerUpdateRequest) UnsetRestartPolicy()`

UnsetRestartPolicy ensures that no value is present for RestartPolicy, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


