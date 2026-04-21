# GpuUtilizationReport

## Properties

Name | Type | Description | Notes
------------ | ------------- | ------------- | -------------
**Index** | **int32** | GPU index on this node | 
**MemoryTotalMb** | **int64** | GPU total memory in MB | 
**MemoryUsedMb** | **int64** | GPU memory currently used in MB | 
**PowerDrawW** | Pointer to **NullableFloat32** | GPU power draw in Watts | [optional] 
**TemperatureC** | Pointer to **NullableInt32** | GPU temperature in Celsius | [optional] 
**UtilizationPercent** | **float32** | GPU compute utilization percentage (0-100) | 

## Methods

### NewGpuUtilizationReport

`func NewGpuUtilizationReport(index int32, memoryTotalMb int64, memoryUsedMb int64, utilizationPercent float32, ) *GpuUtilizationReport`

NewGpuUtilizationReport instantiates a new GpuUtilizationReport object
This constructor will assign default values to properties that have it defined,
and makes sure properties required by API are set, but the set of arguments
will change when the set of required properties is changed

### NewGpuUtilizationReportWithDefaults

`func NewGpuUtilizationReportWithDefaults() *GpuUtilizationReport`

NewGpuUtilizationReportWithDefaults instantiates a new GpuUtilizationReport object
This constructor will only assign default values to properties that have it defined,
but it doesn't guarantee that properties required by API are set

### GetIndex

`func (o *GpuUtilizationReport) GetIndex() int32`

GetIndex returns the Index field if non-nil, zero value otherwise.

### GetIndexOk

`func (o *GpuUtilizationReport) GetIndexOk() (*int32, bool)`

GetIndexOk returns a tuple with the Index field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetIndex

`func (o *GpuUtilizationReport) SetIndex(v int32)`

SetIndex sets Index field to given value.


### GetMemoryTotalMb

`func (o *GpuUtilizationReport) GetMemoryTotalMb() int64`

GetMemoryTotalMb returns the MemoryTotalMb field if non-nil, zero value otherwise.

### GetMemoryTotalMbOk

`func (o *GpuUtilizationReport) GetMemoryTotalMbOk() (*int64, bool)`

GetMemoryTotalMbOk returns a tuple with the MemoryTotalMb field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryTotalMb

`func (o *GpuUtilizationReport) SetMemoryTotalMb(v int64)`

SetMemoryTotalMb sets MemoryTotalMb field to given value.


### GetMemoryUsedMb

`func (o *GpuUtilizationReport) GetMemoryUsedMb() int64`

GetMemoryUsedMb returns the MemoryUsedMb field if non-nil, zero value otherwise.

### GetMemoryUsedMbOk

`func (o *GpuUtilizationReport) GetMemoryUsedMbOk() (*int64, bool)`

GetMemoryUsedMbOk returns a tuple with the MemoryUsedMb field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetMemoryUsedMb

`func (o *GpuUtilizationReport) SetMemoryUsedMb(v int64)`

SetMemoryUsedMb sets MemoryUsedMb field to given value.


### GetPowerDrawW

`func (o *GpuUtilizationReport) GetPowerDrawW() float32`

GetPowerDrawW returns the PowerDrawW field if non-nil, zero value otherwise.

### GetPowerDrawWOk

`func (o *GpuUtilizationReport) GetPowerDrawWOk() (*float32, bool)`

GetPowerDrawWOk returns a tuple with the PowerDrawW field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetPowerDrawW

`func (o *GpuUtilizationReport) SetPowerDrawW(v float32)`

SetPowerDrawW sets PowerDrawW field to given value.

### HasPowerDrawW

`func (o *GpuUtilizationReport) HasPowerDrawW() bool`

HasPowerDrawW returns a boolean if a field has been set.

### SetPowerDrawWNil

`func (o *GpuUtilizationReport) SetPowerDrawWNil(b bool)`

 SetPowerDrawWNil sets the value for PowerDrawW to be an explicit nil

### UnsetPowerDrawW
`func (o *GpuUtilizationReport) UnsetPowerDrawW()`

UnsetPowerDrawW ensures that no value is present for PowerDrawW, not even an explicit nil
### GetTemperatureC

`func (o *GpuUtilizationReport) GetTemperatureC() int32`

GetTemperatureC returns the TemperatureC field if non-nil, zero value otherwise.

### GetTemperatureCOk

`func (o *GpuUtilizationReport) GetTemperatureCOk() (*int32, bool)`

GetTemperatureCOk returns a tuple with the TemperatureC field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetTemperatureC

`func (o *GpuUtilizationReport) SetTemperatureC(v int32)`

SetTemperatureC sets TemperatureC field to given value.

### HasTemperatureC

`func (o *GpuUtilizationReport) HasTemperatureC() bool`

HasTemperatureC returns a boolean if a field has been set.

### SetTemperatureCNil

`func (o *GpuUtilizationReport) SetTemperatureCNil(b bool)`

 SetTemperatureCNil sets the value for TemperatureC to be an explicit nil

### UnsetTemperatureC
`func (o *GpuUtilizationReport) UnsetTemperatureC()`

UnsetTemperatureC ensures that no value is present for TemperatureC, not even an explicit nil
### GetUtilizationPercent

`func (o *GpuUtilizationReport) GetUtilizationPercent() float32`

GetUtilizationPercent returns the UtilizationPercent field if non-nil, zero value otherwise.

### GetUtilizationPercentOk

`func (o *GpuUtilizationReport) GetUtilizationPercentOk() (*float32, bool)`

GetUtilizationPercentOk returns a tuple with the UtilizationPercent field if it's non-nil, zero value otherwise
and a boolean to check if the value has been set.

### SetUtilizationPercent

`func (o *GpuUtilizationReport) SetUtilizationPercent(v float32)`

SetUtilizationPercent sets UtilizationPercent field to given value.



[[Back to Model list]](../README.md#documentation-for-models) [[Back to API list]](../README.md#documentation-for-api-endpoints) [[Back to README]](../README.md)


