# ServiceMetrics

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CpuPercent** | **float64** | CPU usage percentage | 
**MemoryPercent** | **float64** | Memory usage percentage | 
**Rps** | Pointer to **NullableFloat64** | Requests per second | [optional] 

## Methods

### NewServiceMetrics

`func NewServiceMetrics(cpuPercent float64, memoryPercent float64, ) *ServiceMetrics`

NewServiceMetrics instantiates a new ServiceMetrics object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewServiceMetricsWithDefaults

`func NewServiceMetricsWithDefaults() *ServiceMetrics`

NewServiceMetricsWithDefaults instantiates a new ServiceMetrics object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCpuPercent

`func (o *ServiceMetrics) GetCpuPercent() float64`

GetCpuPercent returns the CpuPercent field if non-nil, zero value otherwise.

### GetCpuPercentOk

`func (o *ServiceMetrics) GetCpuPercentOk() (*float64, bool)`

GetCpuPercentOk returns a tuple with the CpuPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuPercent

`func (o *ServiceMetrics) SetCpuPercent(v float64)`

SetCpuPercent sets CpuPercent field to given value.


### GetMemoryPercent

`func (o *ServiceMetrics) GetMemoryPercent() float64`

GetMemoryPercent returns the MemoryPercent field if non-nil, zero value otherwise.

### GetMemoryPercentOk

`func (o *ServiceMetrics) GetMemoryPercentOk() (*float64, bool)`

GetMemoryPercentOk returns a tuple with the MemoryPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryPercent

`func (o *ServiceMetrics) SetMemoryPercent(v float64)`

SetMemoryPercent sets MemoryPercent field to given value.


### GetRps

`func (o *ServiceMetrics) GetRps() float64`

GetRps returns the Rps field if non-nil, zero value otherwise.

### GetRpsOk

`func (o *ServiceMetrics) GetRpsOk() (*float64, bool)`

GetRpsOk returns a tuple with the Rps field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetRps

`func (o *ServiceMetrics) SetRps(v float64)`

SetRps sets Rps field to given value.

### HasRps

`func (o *ServiceMetrics) HasRps() bool`

HasRps returns a boolean if a field has been set.

### SetRpsNil

`func (o *ServiceMetrics) SetRpsNil(b bool)`

 SetRpsNil sets the value for Rps to be an explicit nil

### UnsetRps
`func (o *ServiceMetrics) UnsetRps()`

UnsetRps ensures that no value is present for Rps, not even an explicit nil

[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


