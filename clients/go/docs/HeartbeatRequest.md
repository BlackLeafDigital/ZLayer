# HeartbeatRequest

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**CpuUsed** | **float64** |  | 
**DiskUsed** | **int64** |  | 
**GpuUtilization** | Pointer to [**[]GpuUtilizationReport**](GpuUtilizationReport.md) |  | [optional] 
**MemoryUsed** | **int64** |  | 
**NodeId** | **int64** |  | 

## Methods

### NewHeartbeatRequest

`func NewHeartbeatRequest(cpuUsed float64, diskUsed int64, memoryUsed int64, nodeId int64, ) *HeartbeatRequest`

NewHeartbeatRequest instantiates a new HeartbeatRequest object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewHeartbeatRequestWithDefaults

`func NewHeartbeatRequestWithDefaults() *HeartbeatRequest`

NewHeartbeatRequestWithDefaults instantiates a new HeartbeatRequest object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetCpuUsed

`func (o *HeartbeatRequest) GetCpuUsed() float64`

GetCpuUsed returns the CpuUsed field if non-nil, zero value otherwise.

### GetCpuUsedOk

`func (o *HeartbeatRequest) GetCpuUsedOk() (*float64, bool)`

GetCpuUsedOk returns a tuple with the CpuUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetCpuUsed

`func (o *HeartbeatRequest) SetCpuUsed(v float64)`

SetCpuUsed sets CpuUsed field to given value.


### GetDiskUsed

`func (o *HeartbeatRequest) GetDiskUsed() int64`

GetDiskUsed returns the DiskUsed field if non-nil, zero value otherwise.

### GetDiskUsedOk

`func (o *HeartbeatRequest) GetDiskUsedOk() (*int64, bool)`

GetDiskUsedOk returns a tuple with the DiskUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetDiskUsed

`func (o *HeartbeatRequest) SetDiskUsed(v int64)`

SetDiskUsed sets DiskUsed field to given value.


### GetGpuUtilization

`func (o *HeartbeatRequest) GetGpuUtilization() []GpuUtilizationReport`

GetGpuUtilization returns the GpuUtilization field if non-nil, zero value otherwise.

### GetGpuUtilizationOk

`func (o *HeartbeatRequest) GetGpuUtilizationOk() (*[]GpuUtilizationReport, bool)`

GetGpuUtilizationOk returns a tuple with the GpuUtilization field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetGpuUtilization

`func (o *HeartbeatRequest) SetGpuUtilization(v []GpuUtilizationReport)`

SetGpuUtilization sets GpuUtilization field to given value.

### HasGpuUtilization

`func (o *HeartbeatRequest) HasGpuUtilization() bool`

HasGpuUtilization returns a boolean if a field has been set.

### GetMemoryUsed

`func (o *HeartbeatRequest) GetMemoryUsed() int64`

GetMemoryUsed returns the MemoryUsed field if non-nil, zero value otherwise.

### GetMemoryUsedOk

`func (o *HeartbeatRequest) GetMemoryUsedOk() (*int64, bool)`

GetMemoryUsedOk returns a tuple with the MemoryUsed field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryUsed

`func (o *HeartbeatRequest) SetMemoryUsed(v int64)`

SetMemoryUsed sets MemoryUsed field to given value.


### GetNodeId

`func (o *HeartbeatRequest) GetNodeId() int64`

GetNodeId returns the NodeId field if non-nil, zero value otherwise.

### GetNodeIdOk

`func (o *HeartbeatRequest) GetNodeIdOk() (*int64, bool)`

GetNodeIdOk returns a tuple with the NodeId field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetNodeId

`func (o *HeartbeatRequest) SetNodeId(v int64)`

SetNodeId sets NodeId field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


