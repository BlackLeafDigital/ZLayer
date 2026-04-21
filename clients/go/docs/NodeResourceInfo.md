# NodeResourceInfo

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CpuPercent** | **float64** | CPU usage percentage | 
**CpuTotal** | **float64** | Total CPU cores | 
**CpuUsed** | **float64** | Used CPU cores | 
**MemoryPercent** | **float64** | Memory usage percentage | 
**MemoryTotal** | **int64** | Total memory in bytes | 
**MemoryUsed** | **int64** | Used memory in bytes | 

## Methods

### NewNodeResourceInfo

`func NewNodeResourceInfo(cpuPercent float64, cpuTotal float64, cpuUsed float64, memoryPercent float64, memoryTotal int64, memoryUsed int64, ) *NodeResourceInfo`

NewNodeResourceInfo instantiates a new NodeResourceInfo object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewNodeResourceInfoWithDefaults

`func NewNodeResourceInfoWithDefaults() *NodeResourceInfo`

NewNodeResourceInfoWithDefaults instantiates a new NodeResourceInfo object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCpuPercent

`func (o *NodeResourceInfo) GetCpuPercent() float64`

GetCpuPercent returns the CpuPercent field if non-nil, zero value otherwise.

### GetCpuPercentOk

`func (o *NodeResourceInfo) GetCpuPercentOk() (*float64, bool)`

GetCpuPercentOk returns a tuple with the CpuPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuPercent

`func (o *NodeResourceInfo) SetCpuPercent(v float64)`

SetCpuPercent sets CpuPercent field to given value.


### GetCpuTotal

`func (o *NodeResourceInfo) GetCpuTotal() float64`

GetCpuTotal returns the CpuTotal field if non-nil, zero value otherwise.

### GetCpuTotalOk

`func (o *NodeResourceInfo) GetCpuTotalOk() (*float64, bool)`

GetCpuTotalOk returns a tuple with the CpuTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuTotal

`func (o *NodeResourceInfo) SetCpuTotal(v float64)`

SetCpuTotal sets CpuTotal field to given value.


### GetCpuUsed

`func (o *NodeResourceInfo) GetCpuUsed() float64`

GetCpuUsed returns the CpuUsed field if non-nil, zero value otherwise.

### GetCpuUsedOk

`func (o *NodeResourceInfo) GetCpuUsedOk() (*float64, bool)`

GetCpuUsedOk returns a tuple with the CpuUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuUsed

`func (o *NodeResourceInfo) SetCpuUsed(v float64)`

SetCpuUsed sets CpuUsed field to given value.


### GetMemoryPercent

`func (o *NodeResourceInfo) GetMemoryPercent() float64`

GetMemoryPercent returns the MemoryPercent field if non-nil, zero value otherwise.

### GetMemoryPercentOk

`func (o *NodeResourceInfo) GetMemoryPercentOk() (*float64, bool)`

GetMemoryPercentOk returns a tuple with the MemoryPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryPercent

`func (o *NodeResourceInfo) SetMemoryPercent(v float64)`

SetMemoryPercent sets MemoryPercent field to given value.


### GetMemoryTotal

`func (o *NodeResourceInfo) GetMemoryTotal() int64`

GetMemoryTotal returns the MemoryTotal field if non-nil, zero value otherwise.

### GetMemoryTotalOk

`func (o *NodeResourceInfo) GetMemoryTotalOk() (*int64, bool)`

GetMemoryTotalOk returns a tuple with the MemoryTotal field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryTotal

`func (o *NodeResourceInfo) SetMemoryTotal(v int64)`

SetMemoryTotal sets MemoryTotal field to given value.


### GetMemoryUsed

`func (o *NodeResourceInfo) GetMemoryUsed() int64`

GetMemoryUsed returns the MemoryUsed field if non-nil, zero value otherwise.

### GetMemoryUsedOk

`func (o *NodeResourceInfo) GetMemoryUsedOk() (*int64, bool)`

GetMemoryUsedOk returns a tuple with the MemoryUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryUsed

`func (o *NodeResourceInfo) SetMemoryUsed(v int64)`

SetMemoryUsed sets MemoryUsed field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


