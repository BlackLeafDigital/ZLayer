# ContainerStatsResponse

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CpuUsageUsec** | **int64** | CPU usage in microseconds | 
**Id** | **string** | Container identifier | 
**MemoryBytes** | **int64** | Current memory usage in bytes | 
**MemoryLimit** | **int64** | Memory limit in bytes (&#x60;u64::MAX&#x60; if unlimited) | 
**MemoryPercent** | **float64** | Memory usage as percentage of limit | 

## Methods

### NewContainerStatsResponse

`func NewContainerStatsResponse(cpuUsageUsec int64, id string, memoryBytes int64, memoryLimit int64, memoryPercent float64, ) *ContainerStatsResponse`

NewContainerStatsResponse instantiates a new ContainerStatsResponse object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewContainerStatsResponseWithDefaults

`func NewContainerStatsResponseWithDefaults() *ContainerStatsResponse`

NewContainerStatsResponseWithDefaults instantiates a new ContainerStatsResponse object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCpuUsageUsec

`func (o *ContainerStatsResponse) GetCpuUsageUsec() int64`

GetCpuUsageUsec returns the CpuUsageUsec field if non-nil, zero value otherwise.

### GetCpuUsageUsecOk

`func (o *ContainerStatsResponse) GetCpuUsageUsecOk() (*int64, bool)`

GetCpuUsageUsecOk returns a tuple with the CpuUsageUsec field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuUsageUsec

`func (o *ContainerStatsResponse) SetCpuUsageUsec(v int64)`

SetCpuUsageUsec sets CpuUsageUsec field to given value.


### GetId

`func (o *ContainerStatsResponse) GetId() string`

GetId returns the Id field if non-nil, zero value otherwise.

### GetIdOk

`func (o *ContainerStatsResponse) GetIdOk() (*string, bool)`

GetIdOk returns a tuple with the Id field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetId

`func (o *ContainerStatsResponse) SetId(v string)`

SetId sets Id field to given value.


### GetMemoryBytes

`func (o *ContainerStatsResponse) GetMemoryBytes() int64`

GetMemoryBytes returns the MemoryBytes field if non-nil, zero value otherwise.

### GetMemoryBytesOk

`func (o *ContainerStatsResponse) GetMemoryBytesOk() (*int64, bool)`

GetMemoryBytesOk returns a tuple with the MemoryBytes field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryBytes

`func (o *ContainerStatsResponse) SetMemoryBytes(v int64)`

SetMemoryBytes sets MemoryBytes field to given value.


### GetMemoryLimit

`func (o *ContainerStatsResponse) GetMemoryLimit() int64`

GetMemoryLimit returns the MemoryLimit field if non-nil, zero value otherwise.

### GetMemoryLimitOk

`func (o *ContainerStatsResponse) GetMemoryLimitOk() (*int64, bool)`

GetMemoryLimitOk returns a tuple with the MemoryLimit field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryLimit

`func (o *ContainerStatsResponse) SetMemoryLimit(v int64)`

SetMemoryLimit sets MemoryLimit field to given value.


### GetMemoryPercent

`func (o *ContainerStatsResponse) GetMemoryPercent() float64`

GetMemoryPercent returns the MemoryPercent field if non-nil, zero value otherwise.

### GetMemoryPercentOk

`func (o *ContainerStatsResponse) GetMemoryPercentOk() (*float64, bool)`

GetMemoryPercentOk returns a tuple with the MemoryPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryPercent

`func (o *ContainerStatsResponse) SetMemoryPercent(v float64)`

SetMemoryPercent sets MemoryPercent field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


